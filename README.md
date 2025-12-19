# Gator 🐊

A fast HTTP downloader written in Rust. Uses a work-stealing scheduler to maximize download speeds with parallel segment downloads.

## Features

- **Resume Downloads**: Automatically detects partially downloaded files and continues from where it left off
- **Work-Stealing Scheduler**: Large files are split into 1MB segments and downloaded in parallel by a dynamic worker pool
- **Progress Tracking**: Shows download speed, ETA, and progress in real-time
- **HTTP Range Support**: Uses range requests for efficient parallel downloads
- **Cross-Platform**: Works on Windows, macOS, and Linux

## Installation

You'll need Rust installed. If you don't have it, grab it from [rustup.rs](https://rustup.rs/).

### Build from Source

```bash
git clone <your-repo-url>
cd gator
cargo build --release
```

The binary will be at `target/release/gator`.

### Install Globally

```bash
cargo install --path .
```

## Usage

### Basic Download

```bash
gator https://example.com/largefile.zip
```

### Specify Output File

```bash
gator https://example.com/file.zip -o my-file.zip
```

### Quiet Mode

```bash
gator https://example.com/file.zip --quiet
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

Gator uses a work-stealing scheduler for parallel downloads:

1. **Small Files (<10MB)**: Downloads in a single stream
2. **Large Files (>10MB)**: Splits into 1MB segments and downloads them in parallel using a worker pool (default: max(16, CPU cores × 4))
3. **Resume Support**: Detects existing partial files and continues from the last byte

### Work-Stealing Scheduler

For large files, Gator:
- Splits the file into 1MB segments
- Creates a worker pool that dynamically pulls segments from a queue
- Each worker downloads a segment and writes it directly to the correct file offset
- Fast workers automatically grab more segments, ensuring no idle time
- Pre-allocates the full file size to reduce disk fragmentation

No temporary files are created - everything writes directly to the final file.

## Technical Details

Built with:
- **reqwest**: HTTP client with async DNS
- **tokio**: Async runtime for concurrent downloads
- **mimalloc**: High-performance memory allocator
- **indicatif**: Progress bars

Performance optimizations:
- TCP_NODELAY for lower latency
- File pre-allocation to reduce fragmentation
- Parallel writes with independent file handles
- Work-stealing scheduler for optimal load balancing
- Aggressive LTO compilation for smaller, faster binaries

## Server Compatibility

Works with any HTTP/HTTPS server. Enhanced features require:
- **Range Requests** (`Accept-Ranges: bytes`): Enables parallel downloads and resume
- **Content-Length** header: Enables progress tracking and ETA

## Why "Gator"?

Because it's got a strong bite when it comes to downloading files, and it never lets go until the job is done! 🐊
