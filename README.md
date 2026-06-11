# clank-cli

[![CI](https://github.com/kodokbakar/clank-cli/actions/workflows/ci.yaml/badge.svg)](https://github.com/kodokbakar/clank-cli/actions/workflows/ci.yaml)

HTTP load testing CLI built with Rust. Lightweight, fast, and cross-platform.

## Features

- Concurrent HTTP load testing with configurable workers
- Real-time progress bar with live stats (RPS, latency, errors)
- ETA estimation for duration-based tests
- Color-coded output (green success, red errors)
- Graceful shutdown with Ctrl+C (waits for in-flight requests)
- Multiple output formats: plain text, JSON, CSV
- YAML config file support (`clank.yaml`)
- Custom headers and request body (inline or from file)
- Supports GET, POST, PUT, DELETE, PATCH, HEAD, OPTIONS
- Cross-platform: Linux, macOS, Windows

## Installation

### From source

```bash
cargo install clank-cli
```

### Download binary

Download pre-built binaries from [GitHub Releases](https://github.com/kodokbakar/clank-cli/releases).

### Homebrew

```bash
brew install kodokbakar/tap/clank-cli
```

## Quick Start

```bash
# Basic test (10 workers, stops at Ctrl+C)
clank-cli https://api.example.com

# 100 requests with 20 concurrent workers
clank-cli https://api.example.com -n 100 -c 20

# POST with JSON body
clank-cli https://api.example.com -X POST -T application/json --body '{"name":"test"}'
```

## Usage Examples

```bash
# Duration-based test (run for 30 seconds)
clank-cli https://api.example.com -d 30s

# Custom headers
clank-cli https://api.example.com \
  -H "Authorization: Bearer TOKEN_HERE" \
  -H "Accept: application/json"

# Body from file
clank-cli https://api.example.com -X POST -B request.json

# JSON output for scripting
clank-cli https://api.example.com -n 1000 -o json | jq '.latency.p99_ms'

# CSV output for spreadsheet import
clank-cli https://api.example.com -n 1000 -o csv > results.csv

# Skip config file
clank-cli https://api.example.com --no-config

# Insecure TLS (skip certificate verification)
clank-cli https://localhost:3000 -k

# Quiet mode (no progress bar)
clank-cli https://api.example.com -q
```

### Config file

Create `clank.yaml` in your working directory:

```yaml
url: http://localhost:3000/api
method: POST
body: '{"name": "test"}'
concurrency: 20
timeout_secs: 30
headers:
  - "Authorization: Bearer TOKEN_HERE"
  - "Content-Type: application/json"
```

CLI arguments override config file values. Use `--config <path>` to specify a custom config file, or `--no-config` to skip it entirely.

## CLI Reference

| Flag | Short | Description | Default |
|------|-------|-------------|---------|
| `[URL]` / `--url` | | Target URL (required) | — |
| `-X, --method` | `-X` | HTTP method | GET |
| `-c, --concurrency` | `-c` | Concurrent workers | 10 |
| `-n, --requests` | `-n` | Total requests to send | until Ctrl+C |
| `-d, --duration` | `-d` | Run duration (`5s`, `5m`, `1h30m`) | until Ctrl+C |
| `--body` | | Request body (inline) | — |
| `-B, --body-file` | `-B` | Request body from file | — |
| `-T, --content-type` | `-T` | Content-Type header | — |
| `-H, --header` | `-H` | Custom header (repeatable) | — |
| `--timeout-secs` | | Request timeout in seconds | 10 |
| `-o, --output` | `-o` | Output format (`text`, `json`, `csv`) | text |
| `-f, --config` | `-f` | Config file path | `clank.yaml` |
| `--no-config` | | Skip config file | false |
| `-k, --insecure` | `-k` | Skip TLS verification | false |
| `-q, --quiet` | `-q` | Disable progress bar | false |
| `--no-color` | | Disable colored output | false |
| `--stats-interval-ms` | | Live stats update interval (ms) | 1000 |
| `-h, --help` | `-h` | Print help | |
| `-V, --version` | `-V` | Print version | |

## Output

### Plain text (default)

```
Results:
────────────────────────────────
Total Requests:    1,234
Successful:        1,200 (97.2%)
Errors:            34 (2.8%)
  Timeout:         12
  Connection:      8
  HTTP 4xx:        14
  HTTP 5xx:        0
  HTTP Other:      0
  Other:           0
────────────────────────────────
Latency (avg):     45.2ms
Latency (p50):     42.0ms
Latency (p95):     78.0ms
Latency (p99):     156.7ms
Latency (p999):    230.1ms
Throughput:        123.4 req/s
Duration:          10.00s
────────────────────────────────
```

### JSON (`-o json`)

```json
{
  "total_requests": 1234,
  "successful": 1200,
  "errors": 34,
  "error_rate": 2.8,
  "latency": {
    "avg_ms": 45.2,
    "p50_ms": 42.0,
    "p95_ms": 78.0,
    "p99_ms": 156.7,
    "p999_ms": 230.1
  },
  "throughput_rps": 123.4,
  "duration_secs": 10.0,
  "error_breakdown": {
    "timeout": 12,
    "connection": 8,
    "http_4xx": 14,
    "http_5xx": 0,
    "http_other": 0,
    "other": 0
  }
}
```

### CSV (`-o csv`)

```
total_requests,successful,errors,error_rate,avg_ms,p50_ms,p95_ms,p99_ms,p999_ms,throughput_rps,duration_secs,timeout,connection,http_4xx,http_5xx,http_other,other
1234,1200,34,2.8,45.2,42.0,78.0,156.7,230.1,123.4,10.00,12,8,14,0,0,0
```

## Build from Source

```bash
git clone https://github.com/kodokbakar/clank-cli
cd clank-cli
cargo build --release
```

Binary will be at `target/release/clank-cli`.

For cross-platform builds, see `scripts/build.sh`.

## Tech Stack

- **Runtime:** [Tokio](https://tokio.rs/) (async)
- **HTTP:** [Reqwest](https://docs.rs/reqwest) (rustls-tls)
- **CLI:** [Clap](https://docs.rs/clap) (derive)
- **Latency:** [HdrHistogram](https://docs.rs/hdrhistogram)
- **Progress:** [indicatif](https://docs.rs/indicatif)
- **Output:** [serde](https://docs.rs/serde) + [serde_json](https://docs.rs/serde_json)
- **Config:** [serde_yaml](https://docs.rs/serde_yaml)

## License

[MIT](LICENSE.md) — Copyright (c) 2026 kodokbakar
