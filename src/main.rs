use clap::Parser;
use indicatif::{ProgressBar, ProgressState, ProgressStyle};
use reqwest::header::CONTENT_LENGTH;
use reqwest::{Client, StatusCode};
use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::fs::{File, OpenOptions};
use tokio::sync::{mpsc, Mutex};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use std::fs;
use num_cpus;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser, Debug)]
#[command(name = "gator")]
#[command(author, version, about = "A blazingly fast HTTP downloader", long_about = None)]
struct Args {
    #[arg(required = true)]
    url: String,

    #[arg(short, long)]
    output: Option<String>,

    #[arg(short, long, default_value = "false")]
    quiet: bool,
}

// Segment range for work-stealing scheduler
#[derive(Debug, Clone)]
struct Segment {
    start: u64,
    end: u64,
}

fn create_optimized_client() -> Result<Client, Box<dyn Error + Send + Sync>> {
    // Disable Nagle's algorithm for lower latency
    // reqwest uses async DNS by default, so no custom resolver needed
    let client = Client::builder()
        .tcp_nodelay(true)
        .build()?;
    Ok(client)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let client = Arc::new(create_optimized_client()?);
    let args = Args::parse();

    println!("Fetching {}...", args.url);

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
            download_with_work_stealing(
                client,
                &args.url,
                &file_name,
                starting_pos,
                total_len,
                args.quiet,
            )
            .await?;
        } else {
            download_single_chunk(
                client,
                &args.url,
                &file_name,
                starting_pos,
                total_len,
                args.quiet,
            )
            .await?;
        }
    } else {
        download_single_chunk(client, &args.url, &file_name, starting_pos, 0, args.quiet).await?;
    }

    println!("Download complete!");
    Ok(())
}

async fn download_with_work_stealing(
    client: Arc<Client>,
    url: &str,
    file_name: &str,
    starting_pos: u64,
    total_len: u64,
    quiet: bool,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    const SEGMENT_SIZE: u64 = 1 * 1024 * 1024; // 1MB segments
    let remaining_bytes = total_len - starting_pos;
    let num_segments = ((remaining_bytes as f64) / (SEGMENT_SIZE as f64)).ceil() as u64;

    if !quiet {
        println!(
            "Downloading in {} segments of ~{}MB each using work-stealing scheduler",
            num_segments,
            SEGMENT_SIZE / 1024 / 1024
        );
    }

    // Create work queue for dynamic segment distribution
    let (tx, rx) = mpsc::unbounded_channel::<Segment>();

    let mut current_pos = starting_pos;
    for i in 0..num_segments {
        let start = current_pos;
        let end = if i == num_segments - 1 {
            total_len - 1
        } else {
            current_pos + SEGMENT_SIZE - 1
        };

        tx.send(Segment { start, end })?;
        current_pos = end + 1;
    }
    drop(tx);

    // Share receiver for work-stealing (mutex contention is minimal since workers do async I/O)
    let rx = Arc::new(Mutex::new(rx));

    // Pre-allocate file to reduce fragmentation
    let file_path = Path::new(file_name);
    if !file_path.exists() {
        let file = fs::File::create(file_name)?;
        file.set_len(total_len)?;
    }

    let bytes_downloaded = Arc::new(AtomicU64::new(0));
    let pb = create_progress_bar(
        quiet,
        "Downloading",
        Some(remaining_bytes),
        None,
        bytes_downloaded.clone(),
    );

    // Worker pool size: max(16, CPU * 4)
    let worker_count = std::cmp::max(16, num_cpus::get() * 4);

    if !quiet {
        println!("Spawning {} workers for parallel download", worker_count);
    }

    let mut handles = Vec::new();
    for _ in 0..worker_count {
        let client_clone = client.clone();
        let url = url.to_string();
        let file_name = file_name.to_string();
        let rx = rx.clone();
        let pb = pb.clone();
        let bytes_downloaded = bytes_downloaded.clone();

        let handle = tokio::spawn(async move {
            // Each worker has its own file handle for parallel writes
            let mut file = OpenOptions::new()
                .write(true)
                .read(false)
                .open(&file_name)
                .await?;

            loop {
                // Pull next segment from queue (work-stealing)
                let segment = {
                    let mut rx_guard = rx.lock().await;
                    rx_guard.recv().await
                };

                let segment = match segment {
                    Some(seg) => seg,
                    None => break,
                };

                let range_header = format!("bytes={}-{}", segment.start, segment.end);
                let mut response = client_clone
                    .get(&url)
                    .header("Range", range_header)
                    .send()
                    .await?;

                if !response.status().is_success() && response.status() != StatusCode::PARTIAL_CONTENT {
                    return Err(format!("Segment download failed: {}", response.status()).into());
                }

                // Write directly to correct file offset
                file.seek(std::io::SeekFrom::Start(segment.start)).await?;

                while let Some(chunk) = response.chunk().await? {
                    file.write_all(&chunk).await?;
                    let chunk_len = chunk.len() as u64;
                    bytes_downloaded.fetch_add(chunk_len, Ordering::Relaxed);
                    pb.inc(chunk_len);
                }
            }

            Ok::<(), Box<dyn Error + Send + Sync>>(())
        });

        handles.push(handle);
    }

    let results = futures::future::join_all(handles).await;

    for result in results {
        match result {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => return Err(e),
            Err(e) => return Err(Box::new(e)),
        }
    }

    pb.finish_with_message("Download complete!");
    Ok(())
}

async fn download_single_chunk(
    client: Arc<Client>,
    url: &str,
    file_name: &str,
    starting_pos: u64,
    total_len: u64,
    quiet: bool,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let bytes_downloaded = Arc::new(AtomicU64::new(0));
    let pb = if total_len > 0 {
        create_progress_bar(
            quiet,
            "Downloading",
            Some(total_len - starting_pos),
            None,
            bytes_downloaded.clone(),
        )
    } else {
        create_progress_bar(
            quiet,
            "Downloading",
            None,
            None,
            bytes_downloaded.clone(),
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

    // Pre-allocate file if we know the size
    if total_len > 0 && starting_pos == 0 {
        let file = fs::File::create(file_name)?;
        file.set_len(total_len)?;
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
        let chunk_len = chunk.len() as u64;
        bytes_downloaded.fetch_add(chunk_len, Ordering::Relaxed);
        pb.inc(chunk_len);
    }

    pb.finish_with_message("Download complete!");
    Ok(())
}

fn create_progress_bar(
    quiet: bool,
    msg: &str,
    length: Option<u64>,
    _num_chunks: Option<u64>,
    _bytes_downloaded: Arc<AtomicU64>,
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
        false => {
            bar.set_style(ProgressStyle::default_spinner());
        }
    };

    bar
}
