# Gator 🐊

A blazingly fast HTTP downloader written in Rust with support for resumable downloads, chunked downloading, and real-time progress tracking.

## Features

- **Resume Downloads**: Automatically detects partially downloaded files and continues from where it left off
- **Chunked Downloads**: Large files (>10MB) are downloaded in parallel chunks for maximum speed
- **Progress Tracking**: Beautiful progress bars showing download speed, ETA, and chunk completion
- **HTTP Range Support**: Leverages server range requests for efficient partial downloads
- **Cross-Platform**: Works on Windows, macOS, and Linux
- **Zero Configuration**: Just point it at a URL and watch it work

## Installation

### Prerequisites

Make sure you have Rust installed. If not, get it from [rustup.rs](https://rustup.rs/).

### Build from Source

```bash
git clone <your-repo-url>
cd gator
cargo build --release
```

The binary will be available at `target/release/rget-clone`.

### Install Globally

```bash
cargo install --path .
```

## Usage

### Basic Download

Download a file to the current directory:

```bash
gator https://example.com/largefile.zip
```

### Specify Output File

Download with a custom filename:

```bash
gator https://example.com/file.zip -o my-file.zip
# or
gator https://example.com/file.zip --output my-file.zip
```

### Quiet Mode

Download without progress bars (useful for scripts):

```bash
gator https://example.com/file.zip --quiet
# or
gator https://example.com/file.zip -q
```

### Command Line Options

```
USAGE:
    gator <URL> [OPTIONS]

ARGUMENTS:
    <URL>    The URL to download from

OPTIONS:
    -o, --output <FILE>    Output filename (defaults to the last part of the URL)
    -q, --quiet           Suppress progress output
    -h, --help            Print help information
    -V, --version         Print version information
```

## How It Works

### Smart Download Strategy

Gator automatically determines the best download strategy based on the file:

1. **Small Files (<10MB)**: Downloads in a single stream
2. **Large Files (>10MB)**: Splits into 10MB chunks and downloads in parallel
3. **Resume Support**: Detects existing partial files and continues from the last byte

### Chunked Downloads

For large files on servers that support range requests, Gator:

- Splits the file into ~10MB chunks
- Downloads all chunks concurrently
- Shows per-chunk progress in the status bar
- Automatically combines chunks into the final file
- Cleans up temporary chunk files

### Progress Tracking

The progress bar shows:
- Current download speed (MB/s)
- Bytes downloaded / Total bytes
- Estimated time remaining (ETA)
- Chunk completion status (for chunked downloads)
- Elapsed time

## Technical Details

### Dependencies

- **reqwest**: HTTP client with streaming support
- **tokio**: Async runtime for concurrent downloads
- **clap**: Command-line argument parsing
- **indicatif**: Progress bar implementation
- **futures**: Async utilities for handling concurrent tasks

### Performance

- Concurrent chunk downloads maximize bandwidth utilization
- Streaming writes prevent memory bloat on large files
- Range requests minimize server load and enable resume functionality
- Progress tracking has minimal performance overhead

### Server Compatibility

Gator works with any HTTP/HTTPS server, with enhanced features when the server supports:
- **Range Requests** (`Accept-Ranges: bytes`): Enables chunked downloads and resume
- **Content-Length** header: Enables progress tracking and ETA calculation
- **Content-Type** header: Shows file type information

## Error Handling

Gator gracefully handles common scenarios:

- **Network interruptions**: Automatic resume on restart
- **Server errors**: Clear error messages with HTTP status codes
- **Disk space issues**: Fails gracefully with filesystem errors
- **Invalid URLs**: Validates URLs before attempting download
- **Permission errors**: Clear messages for file system permission issues

## Why "Gator"?

Because it's got a strong bite when it comes to downloading files, and it never lets go until the job is done! 🐊
