# clank-cli

[![CI](https://github.com/kodokbakar/clank-cli/actions/workflows/ci.yaml/badge.svg)](https://github.com/kodokbakar/clank-cli/actions/workflows/ci.yaml)
[![crates.io](https://img.shields.io/crates/v/clank-cli.svg)](https://crates.io/crates/clank-cli)

HTTP load testing CLI built with Rust. Fast, lightweight, and cross-platform.

`clank-cli` helps you benchmark HTTP endpoints with concurrent workers, live stats, retries, rate limiting, response validation, and shareable HTML reports.

## Features

* Concurrent HTTP load testing
* Request-based, duration-based, or continuous mode
* Live terminal stats with progress bar
* Output formats: text, JSON, CSV, HTML
* Self-contained HTML reports with charts and dark theme
* Rate limiting: `100/s`, `5000/m`, `10000/h`
* Ramp-up mode for gradual traffic growth
* Retry support for transient failures
* HTTP keep-alive control
* Response validation for status, body, and headers
* YAML config file support
* Custom method, headers, body, body file, timeout, and insecure TLS mode

## Installation

```bash
cargo install clank-cli
```

Or download binaries from [GitHub Releases](https://github.com/kodokbakar/clank-cli/releases).

Homebrew:

```bash
brew install kodokbakar/tap/clank-cli
```

## Quick Start

```bash
# Run until Ctrl+C with default 10 workers
clank-cli https://api.example.com

# Send 100 requests with 20 workers
clank-cli https://api.example.com -n 100 -c 20

# Run for 30 seconds
clank-cli https://api.example.com -d 30s

# POST JSON
clank-cli https://api.example.com \
  -X POST \
  -T application/json \
  --body '{"name":"test"}'
```

## Common Examples

```bash
# Custom headers
clank-cli https://api.example.com \
  -H "Authorization: Bearer TOKEN_HERE" \
  -H "Accept: application/json"

# Request body from file
clank-cli https://api.example.com -X POST -B request.json

# JSON output for scripts
clank-cli https://api.example.com -n 1000 -o json | jq '.latency.p99_ms'

# CSV output
clank-cli https://api.example.com -n 1000 -o csv > results.csv

# HTML report
clank-cli https://api.example.com -n 1000 -o html

# HTML report with custom path
clank-cli https://api.example.com -n 1000 -o html --output-file reports/api.html
```

## Traffic Control

Use these flags to shape how traffic is sent.

| Feature        | Flag                | Example               |
| -------------- | ------------------- | --------------------- |
| Concurrency    | `-c, --concurrency` | `-c 50`               |
| Rate limit     | `-r, --rate-limit`  | `--rate-limit 100/s`  |
| Ramp-up        | `--ramp-up`         | `--ramp-up 10s`       |
| Ramp-up step   | `--ramp-up-step`    | `--ramp-up-step 5`    |
| Retry          | `--retry`           | `--retry 3`           |
| Retry delay    | `--retry-delay`     | `--retry-delay 100ms` |
| Keep-alive off | `--no-keep-alive`   | `--no-keep-alive`     |

Examples:

```bash
# Limit to 100 requests per second
clank-cli https://api.example.com -d 30s --rate-limit 100/s

# Gradually start 20 workers over 10 seconds
clank-cli https://api.example.com -c 20 --ramp-up 10s -d 30s

# Add 5 workers per ramp-up step
clank-cli https://api.example.com -c 20 --ramp-up 10s --ramp-up-step 5

# Retry transient 5xx, connection, and timeout failures
clank-cli https://api.example.com --retry 3 --retry-delay 100ms -d 30s

# Combine rate limit, ramp-up, and retry
clank-cli https://api.example.com \
  -c 20 \
  --ramp-up 10s \
  --rate-limit 100/s \
  --retry 3 \
  --retry-delay 100ms \
  -d 30s
```

Notes:

* Rate limiting controls request timing, not worker count.
* Ramp-up only controls when workers start. Final concurrency stays the same.
* `--retry 3` means one initial attempt plus up to 3 retries.
* HTTP 5xx, connection errors, and timeouts are retried. HTTP 4xx is not retried.
* Keep-alive is enabled by default. Use `--no-keep-alive` to benchmark cold connections.

## Response Validation

Use validation flags to make sure responses still match the expected contract during a load test.

```bash
# Require HTTP 200
clank-cli http://localhost:8080 --expect-status 200

# Allow multiple status codes
clank-cli http://localhost:8080 --expect-status 200,201,204

# Require body pattern
clank-cli http://localhost:8080 --expect-body '"status":"ok"'

# Require response header
clank-cli http://localhost:8080 --expect-header "Content-Type: application/json"

# Combine validation with retry and rate limit
clank-cli http://localhost:8080 \
  --expect-status 200 \
  --expect-body "ok" \
  --retry 3 \
  --rate-limit 100/s \
  -d 30s
```

Validation failures are counted as errors and increment `validation_errors` in text, JSON, CSV, HTML report, and live stats.

When validation fails, `clank-cli` prints the selected summary first, then prints validation details to stderr and exits with status code `1`.

## HTML Reports

Use `-o html` or `--output html` to generate a self-contained report file.

```bash
# Auto-generated file name
clank-cli http://localhost:8080 -n 1000 -c 20 -o html

# Custom file path
clank-cli http://localhost:8080 -n 1000 -c 20 \
  -o html \
  --output-file reports/load-test.html
```

Example stdout:

```text
HTML report written to http-localhost-8080-report-1700000000.html
```

The report includes:

* Summary cards
* Latency percentiles
* Latency histogram
* Status code breakdown
* Error distribution
* Response validation details
* Target URL, command, duration, version, and rate limit metadata

The HTML file embeds CSS, JavaScript, report data, and Chart.js, so it works offline.

## Config File

Create `clank.yaml` in your working directory:

```yaml
url: http://localhost:3000/api
method: POST
body: '{"name":"test"}'
concurrency: 20
timeout_secs: 30
rate_limit: 100/s
headers:
  - "Authorization: Bearer TOKEN_HERE"
  - "Content-Type: application/json"
```

Run with config:

```bash
clank-cli
```

Useful config flags:

```bash
# Use a custom config file
clank-cli --config ./loadtest.yaml

# Ignore clank.yaml
clank-cli --no-config
```

CLI arguments override config values.

## Output Formats

### Text

Default human-readable terminal summary.

```bash
clank-cli http://localhost:8080 -n 100
```

### JSON

Useful for automation and scripts.

```bash
clank-cli http://localhost:8080 -n 100 -o json
```

Example fields:

```json
{
  "total_requests": 100,
  "validation_errors": 0,
  "retries": 0,
  "successful": 100,
  "errors": 0,
  "error_rate": 0.0,
  "latency": {
    "avg_ms": 12.3,
    "p50_ms": 10.0,
    "p95_ms": 20.0,
    "p99_ms": 30.0,
    "p999_ms": 40.0
  },
  "throughput_rps": 250.0,
  "rate_limit": "unlimited",
  "duration_secs": 0.4
}
```

### CSV

Useful for spreadsheets.

```bash
clank-cli http://localhost:8080 -n 100 -o csv > results.csv
```

### HTML

Useful for sharing visual reports.

```bash
clank-cli http://localhost:8080 -n 100 -o html --output-file report.html
```

## CLI Reference

| Flag                           | Description                                    | Default                            |
| ------------------------------ | ---------------------------------------------- | ---------------------------------- |
| `[URL]`, `--url <URL>`         | Target URL                                     | required unless config provides it |
| `-X, --method <METHOD>`        | HTTP method                                    | `GET`                              |
| `-c, --concurrency <N>`        | Concurrent workers                             | `10`                               |
| `-n, --requests <N>`           | Total requests                                 | until Ctrl+C                       |
| `-d, --duration <DURATION>`    | Run duration, for example `30s`, `5m`, `1h30m` | until Ctrl+C                       |
| `-r, --rate-limit <RATE>`      | Limit request rate, for example `100/s`        | unlimited                          |
| `--ramp-up <DURATION>`         | Gradual worker startup duration                | disabled                           |
| `--ramp-up-step <N>`           | Workers added per ramp-up step                 | `1`                                |
| `--retry <N>`                  | Retry failed requests up to N times            | `0`                                |
| `--retry-delay <DURATION>`     | Delay between retries                          | `0ms`                              |
| `--body <BODY>`                | Inline request body                            | none                               |
| `-B, --body-file <FILE>`       | Request body from file                         | none                               |
| `-T, --content-type <TYPE>`    | Set `Content-Type` header                      | none                               |
| `-H, --header <KEY: VALUE>`    | Custom request header, repeatable              | none                               |
| `--expect-status <CODES>`      | Validate status code, comma-separated          | none                               |
| `--expect-body <PATTERN>`      | Validate response body with regex              | none                               |
| `--expect-header <KEY: VALUE>` | Validate response header, repeatable           | none                               |
| `--timeout-secs <N>`           | Request timeout in seconds                     | `10`                               |
| `-o, --output <FORMAT>`        | `text`, `json`, `csv`, or `html`               | `text`                             |
| `--output-file <PATH>`         | HTML report path, only with `--output html`    | auto-generated                     |
| `-f, --config <FILE>`          | Config file path                               | `clank.yaml`                       |
| `--no-config`                  | Skip config file                               | `false`                            |
| `-k, --insecure`               | Disable TLS certificate verification           | `false`                            |
| `--keep-alive`                 | Enable HTTP connection reuse                   | enabled                            |
| `--no-keep-alive`              | Disable HTTP connection reuse                  | disabled                           |
| `-q, --quiet`                  | Disable progress bar                           | `false`                            |
| `--no-color`                   | Disable colored output                         | `false`                            |
| `--stats-interval-ms <N>`      | Live stats update interval                     | `1000`                             |
| `-h, --help`                   | Print help                                     |                                    |
| `-V, --version`                | Print version                                  |                                    |

## Build from Source

```bash
git clone https://github.com/kodokbakar/clank-cli
cd clank-cli
cargo build --release
```

Binary path:

```bash
target/release/clank-cli
```

## Tech Stack

* Tokio
* Reqwest with rustls
* Clap
* serde, serde_json, serde_yaml
* HdrHistogram
* indicatif
* Chart.js for HTML reports

## License

[MIT](LICENSE.md) — Copyright (c) 2026 kodokbakar
