use clap::Parser;
use indicatif::{ProgressBar, ProgressState, ProgressStyle};
use reqwest::header::CONTENT_LENGTH;
use reqwest::{Client, StatusCode};
use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::fs::{File, OpenOptions};
use tokio::task::JoinHandle;

#[allow(unused_imports)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(required = true)]
    url: String,

    #[arg(short, long)]
    output: Option<String>,

    #[arg(short, long, default_value = "false")]
    quiet: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let client = Client::new();
    let args = Args::parse();

    println!("Fetching {}...", args.url);

    // Determine file name
    let file_name = if let Some(output_dest) = args.output {
        output_dest
    } else {
        args.url
            .split('/')
            .last()
            .unwrap_or("downloaded_file")
            .to_string()
    };

    let file_path = Path::new(&file_name);
    let mut starting_pos = 0;

    if file_path.exists() {
        let existing_file = File::open(&file_path).await?;
        starting_pos = existing_file.metadata().await?.len();
        println!(
            "Existing file found, attempting to resume download from byte {}...",
            starting_pos
        );
    } else {
        println!("Starting new download...");
    }

    // Get file info with HEAD request
    let head_response = client.head(&args.url).send().await?;

    if !args.quiet {
        println!("HTTP request sent... {}", head_response.status());
    }

    if !head_response.status().is_success() {
        return Err(format!("Server returned error: {}", head_response.status()).into());
    }

    let headers = head_response.headers();
    let content_length = headers
        .get(CONTENT_LENGTH)
        .and_then(|ct_len| ct_len.to_str().ok())
        .and_then(|ct_len| ct_len.parse::<u64>().ok());

    let content_type = headers
        .get("content-type")
        .and_then(|ct| ct.to_str().ok())
        .unwrap_or("unknown");

    match content_length {
        Some(len) => {
            if !args.quiet {
                println!("Length: {} bytes", len);
                println!("Type: {}", content_type);
            }
        }
        None => {
            if !args.quiet {
                println!("Length: unknown");
            }
        }
    }

    let accepts_ranges = headers
        .get("accept-ranges")
        .and_then(|h| h.to_str().ok())
        .map(|s| s == "bytes")
        .unwrap_or(false);

    if let Some(total_len) = content_length {
        if accepts_ranges && total_len > 10 * 1024 * 1024 && starting_pos < total_len {
            download_with_chunks(
                &client,
                &args.url,
                &file_name,
                starting_pos,
                total_len,
                args.quiet,
            )
            .await?;
        } else {
            download_single_chunk(
                &client,
                &args.url,
                &file_name,
                starting_pos,
                total_len,
                args.quiet,
            )
            .await?;
        }
    } else {
        download_single_chunk(&client, &args.url, &file_name, starting_pos, 0, args.quiet).await?;
    }

    println!("Download complete!");
    Ok(())
}

async fn download_with_chunks(
    client: &Client,
    url: &str,
    file_name: &str,
    starting_pos: u64,
    total_len: u64,
    quiet: bool,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let chunk_size = 10 * 1024 * 1024; // 10MB chunks
    let remaining_bytes = total_len - starting_pos;
    let num_chunks = ((remaining_bytes as f64) / (chunk_size as f64)).ceil() as u64;

    if !quiet {
        println!(
            "Downloading in {} chunks of ~{}MB each",
            num_chunks,
            chunk_size / 1024 / 1024
        );
    }

    let chunks_completed = Arc::new(AtomicU64::new(0));
    let pb = create_progress_bar(
        quiet,
        "Downloading",
        Some(remaining_bytes),
        Some(num_chunks),
        chunks_completed.clone(),
    );

    let mut tasks = Vec::new();
    let mut current_pos = starting_pos;

    for i in 0..num_chunks {
        let start = current_pos;
        let end = if i == num_chunks - 1 {
            total_len - 1
        } else {
            current_pos + chunk_size - 1
        };

        let temp_file_name = format!("{}.part{}", file_name, i);
        let task_url = url.to_string();
        let task_client = client.clone();
        let pb_clone = pb.clone();
        let chunks_completed_clone = chunks_completed.clone();

        let task: JoinHandle<Result<(), Box<dyn Error + Send + Sync>>> = tokio::spawn(async move {
            download_chunk(
                &task_client,
                &task_url,
                start,
                end,
                &temp_file_name,
                pb_clone,
                chunks_completed_clone,
            )
            .await
        });

        tasks.push(task);
        current_pos = end + 1;
    }

    // Wait for all chunks to complete
    let results = futures::future::join_all(tasks).await;

    for result in results {
        match result {
            Ok(Ok(_)) => {} // Success
            Ok(Err(e)) => return Err(e),
            Err(e) => return Err(Box::new(e)),
        }
    }

    pb.finish_with_message("Chunks downloaded, combining...");

    // Combine all parts into final file
    let mut final_file = if starting_pos > 0 {
        OpenOptions::new()
            .write(true)
            .append(true)
            .open(file_name)
            .await?
    } else {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(file_name)
            .await?
    };

    for i in 0..num_chunks {
        let temp_file_name = format!("{}.part{}", file_name, i);
        if Path::new(&temp_file_name).exists() {
            let mut temp_file = File::open(&temp_file_name).await?;
            tokio::io::copy(&mut temp_file, &mut final_file).await?;
            tokio::fs::remove_file(&temp_file_name).await?;
        }
    }

    Ok(())
}

async fn download_single_chunk(
    client: &Client,
    url: &str,
    file_name: &str,
    starting_pos: u64,
    total_len: u64,
    quiet: bool,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let pb = if total_len > 0 {
        create_progress_bar(
            quiet,
            "Downloading",
            Some(total_len - starting_pos),
            None,
            Arc::new(AtomicU64::new(0)),
        )
    } else {
        create_progress_bar(
            quiet,
            "Downloading",
            None,
            None,
            Arc::new(AtomicU64::new(0)),
        )
    };

    let mut request = client.get(url);

    if starting_pos > 0 {
        request = request.header("Range", format!("bytes={}-", starting_pos));
    }

    let mut response = request.send().await?;

    if !response.status().is_success() && response.status() != StatusCode::PARTIAL_CONTENT {
        return Err(format!("Server returned error: {}", response.status()).into());
    }

    let mut file = if starting_pos > 0 {
        OpenOptions::new()
            .write(true)
            .append(true)
            .open(file_name)
            .await?
    } else {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(file_name)
            .await?
    };

    while let Some(chunk) = response.chunk().await? {
        file.write_all(&chunk).await?;
        pb.inc(chunk.len() as u64);
    }

    pb.finish_with_message("Download complete!");
    Ok(())
}

async fn download_chunk(
    client: &Client,
    url: &str,
    start_byte: u64,
    end_byte: u64,
    temp_file_name: &str,
    pb: ProgressBar,
    chunks_completed: Arc<AtomicU64>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let range_header = format!("bytes={}-{}", start_byte, end_byte);
    let mut response = client.get(url).header("Range", range_header).send().await?;

    if !response.status().is_success() && response.status() != StatusCode::PARTIAL_CONTENT {
        return Err(format!("Chunk download failed: {}", response.status()).into());
    }

    let mut temp_file = File::create(temp_file_name).await?;

    while let Some(chunk) = response.chunk().await? {
        temp_file.write_all(&chunk).await?;
        pb.inc(chunk.len() as u64);
    }

    chunks_completed.fetch_add(1, Ordering::SeqCst);

    Ok(())
}

fn create_progress_bar(
    quiet: bool,
    msg: &str,
    length: Option<u64>,
    num_chunks: Option<u64>,
    chunks_completed: Arc<AtomicU64>,
) -> ProgressBar {
    let bar = match quiet {
        true => ProgressBar::hidden(),
        false => match length {
            Some(len) => ProgressBar::new(len),
            None => ProgressBar::new_spinner(),
        },
    };

    bar.set_message(msg.to_string());

    match length.is_some() {
        true => {
            if let Some(total_chunks) = num_chunks {
                let chunks_completed_clone = chunks_completed.clone();
                bar.set_style(ProgressStyle::default_bar()
                    .template("{msg} {spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {chunks} {speed} {bytes}/{total_bytes} eta: {eta}")
                    .unwrap()
                    .with_key("chunks", move |_state: &ProgressState, w: &mut dyn std::fmt::Write| {
                        let completed = chunks_completed_clone.load(Ordering::SeqCst);
                        write!(w, "{}/{}", completed, total_chunks).unwrap();
                    })
                    .with_key("speed", |state: &ProgressState, w: &mut dyn std::fmt::Write| {
                        let bytes_per_sec = state.per_sec();
                        let mb_per_sec = bytes_per_sec / (1024.0 * 1024.0);
                        write!(w, "{:.2} MB/s", mb_per_sec).unwrap();
                    })
                    .progress_chars("=> "));
            } else {
                bar.set_style(ProgressStyle::default_bar()
                    .template("{msg} {spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {speed} {bytes}/{total_bytes} eta: {eta}")
                    .unwrap()
                    .with_key("speed", |state: &ProgressState, w: &mut dyn std::fmt::Write| {
                        let bytes_per_sec = state.per_sec();
                        let mb_per_sec = bytes_per_sec / (1024.0 * 1024.0);
                        write!(w, "{:.2} MB/s", mb_per_sec).unwrap();
                    })
                    .progress_chars("=> "));
            }
        }
        false => {
            bar.set_style(ProgressStyle::default_spinner());
        }
    };

    bar
}
