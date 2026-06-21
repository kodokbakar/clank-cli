# clank-cli

[![CI](https://github.com/kodokbakar/clank-cli/actions/workflows/ci.yaml/badge.svg)](https://github.com/kodokbakar/clank-cli/actions/workflows/ci.yaml)
[![crates.io](https://img.shields.io/crates/v/clank-cli.svg)](https://crates.io/crates/clank-cli)

HTTP load testing CLI built with Rust. Lightweight, fast, and cross-platform.

## Features

- Concurrent HTTP load testing with configurable workers
- Real-time progress bar with live stats (RPS, latency, errors)
- ETA estimation for duration-based tests
- Color-coded output (green success, red errors)
- Graceful shutdown with Ctrl+C (waits for in-flight requests)
- Multiple output formats: plain text, JSON, CSV, and self-contained HTML reports
- YAML config file support (`clank.yaml`)
- Custom headers and request body (inline or from file)
- Supports GET, POST, PUT, DELETE, PATCH, HEAD, OPTIONS
- Cross-platform: Linux, macOS, Windows
- Rate limiting with `--rate-limit` (`N/s`, `N/m`, `N/h`)
- Ramp-up mode with `--ramp-up` and `--ramp-up-step`
- Retry support with `--retry` and `--retry-delay`
- Keep-alive control with `--keep-alive` and `--no-keep-alive`
- Response validation with `--expect-status`, `--expect-body`, and `--expect-header`
- Validation error stats in plain text, JSON, CSV, HTML report, and live terminal output
- Retry stats in plain text, JSON, CSV, and HTML report output
- HTML report output with charts, dark theme, and offline browser support

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

# HTML report with charts
clank-cli https://api.example.com -n 1000 -o html

# HTML report with custom output path
clank-cli https://api.example.com -n 1000 -o html --output-file reports/api-report.html

# Skip config file
clank-cli https://api.example.com --no-config

# Insecure TLS (skip certificate verification)
clank-cli https://localhost:3000 -k

# Quiet mode (no progress bar)
clank-cli https://api.example.com -q

# Limit to 100 requests per second
clank-cli https://api.example.com --rate-limit 100/s --duration 30s

# Limit to 5000 requests per minute
clank-cli https://api.example.com -r 5000/m -d 1m

# Gradually ramp up from 1 worker to 10 workers over 10 seconds
clank-cli https://api.example.com --concurrency 10 --ramp-up 10s --duration 30s

# Add 5 workers per ramp-up step until reaching 20 workers
clank-cli https://api.example.com --concurrency 20 --ramp-up 10s --ramp-up-step 5 --duration 30s

# Rate limit with ramp-up to avoid cold-start burst
clank-cli https://api.example.com \
  --concurrency 20 \
  --ramp-up 10s \
  --rate-limit 100/s \
  --duration 30s

# Rapid ramp-up for a quick test
clank-cli https://api.example.com --concurrency 50 --ramp-up 5s --duration 30s

# Retry transient 5xx, connection, or timeout errors
clank-cli https://api.example.com --retry 3 --retry-delay 100ms --duration 30s

# Disable keep-alive to benchmark cold connections
clank-cli https://api.example.com --no-keep-alive -n 100

# Retry + rate limiting + ramp-up
clank-cli https://api.example.com \
  --concurrency 20 \
  --ramp-up 10s \
  --rate-limit 100/s \
  --retry 3 \
  --retry-delay 100ms \
  --duration 30s

# Response validation + retry + rate limiting
clank-cli https://api.example.com \
  --expect-status 200,201 \
  --expect-body '"status":"ok"' \
  --expect-header "Content-Type: application/json" \
  --retry 3 \
  --rate-limit 100/s \
  --duration 30s
```

### Config file

Create `clank.yaml` in your working directory:

```yaml
url: http://localhost:3000/api
method: POST
body: '{"name": "test"}'
concurrency: 20
timeout_secs: 30
rate_limit: 100/s
headers:
  - "Authorization: Bearer TOKEN_HERE"
  - "Content-Type: application/json"
```

CLI arguments override config file values. Use `--config <path>` to specify a custom config file, or `--no-config` to skip it entirely.

## Rate Limiting

Use `--rate-limit` or `-r` to throttle request throughput across all workers.

Supported formats:

| Format    | Meaning                  |
| --------- | ------------------------ |
| `100/s`   | 100 requests per second  |
| `5000/m`  | 5000 requests per minute |
| `10000/h` | 10000 requests per hour  |

Examples:

```bash
# Limit to 100 requests per second
clank-cli --url http://localhost:8080 --rate-limit 100/s --duration 30s

# Limit to 5000 requests per minute
clank-cli --url http://localhost:8080 -r 5000/m -d 1m
```

You can also configure rate limiting in `clank.yaml`:

```yaml
rate_limit: 5000/m
```

CLI arguments override config file values, so `--rate-limit` takes priority over `rate_limit` in `clank.yaml`.

Rate limiting does not reduce concurrent connections. It only throttles request timing across all concurrent workers.

## Ramp-Up

Use `--ramp-up <DURATION>` to increase concurrency gradually instead of starting all workers at once.

Ramp-up is useful when you want a more realistic load test. Real users usually do not arrive at the exact same millisecond, so starting all workers instantly can create an artificial burst at the beginning of the test. That burst can overwhelm a target server before the steady-state load is reached.

Without ramp-up, `clank-cli` starts all workers immediately. This is still the default behavior and remains backward compatible.

Use ramp-up when:

* You want traffic to grow gradually.
* You are testing warm-up behavior.
* You want to avoid a burst in the first second.
* You are combining high concurrency with rate limiting.

Skip ramp-up when:

* You intentionally want an instant burst test.
* You are doing a very small local smoke test.
* You only care about maximum immediate pressure.

Examples:

```bash
# Gradually increase to 10 workers over 10 seconds
clank-cli http://localhost:8080 -c 10 --ramp-up 10s

# Add 5 workers per step until reaching 20 workers
clank-cli http://localhost:8080 -c 20 --ramp-up 10s --ramp-up-step 5

# Rapid ramp-up for a quick test
clank-cli http://localhost:8080 -c 50 --ramp-up 5s

# Combine ramp-up with rate limiting
clank-cli http://localhost:8080 -c 20 --ramp-up 10s --rate-limit 100/s --duration 30s
```

Ramp-up only controls when workers are started. It does not change the final target concurrency.

The step interval is calculated as:

```text
ramp_up_duration / ceil(concurrency / ramp_up_step)
```

Examples:

| Command                                | Step behavior                                          |
| -------------------------------------- | ------------------------------------------------------ |
| `-c 10 --ramp-up 10s`                  | 10 steps, 1 worker per step, 1 second between steps    |
| `-c 20 --ramp-up 10s --ramp-up-step 5` | 4 steps, 5 workers per step, 2.5 seconds between steps |
| `-c 50 --ramp-up 5s --ramp-up-step 10` | 5 steps, 10 workers per step, 1 second between steps   |

If `--ramp-up` is omitted, all workers start immediately. If `--ramp-up 0s` is used, ramp-up is treated as disabled.

`--ramp-up-step` controls how many workers are added per step. The default is `1`, and the value must be greater than `0`.

## Retry

Use `--retry <N>` to retry failed requests when the failure is likely transient.

Retry is useful when:

* The target server occasionally returns HTTP 5xx.
* A connection fails temporarily.
* A request times out under load.
* You want to measure how often the target recovers after transient failures.

Examples:

```bash
# Retry failed requests up to 3 times
clank-cli http://localhost:8080 --retry 3

# Retry with 100ms delay between attempts
clank-cli http://localhost:8080 --retry 3 --retry-delay 100ms

# Combine retry with rate limiting and ramp-up
clank-cli http://localhost:8080 \
  --concurrency 20 \
  --ramp-up 10s \
  --rate-limit 100/s \
  --retry 3 \
  --retry-delay 100ms \
  --duration 30s
```

Retry behavior:

| Failure type                          | Retried? |
| ------------------------------------- | -------- |
| HTTP 5xx                              | Yes      |
| Connection error                      | Yes      |
| Timeout                               | Yes      |
| HTTP 4xx                              | No       |
| Invalid request or unsupported method | No       |

`--retry 3` means one initial attempt plus up to 3 retry attempts. The default is `--retry 0`, so retry is disabled unless explicitly configured.

`--retry-delay <DURATION>` controls the delay between retry attempts. The default is `0ms`.

Retry counts are included in plain text, JSON, CSV, and live terminal stats.

## Keep-alive

By default, `clank-cli` uses HTTP keep-alive so connections can be reused across multiple requests. This is usually the best default because it matches common production client behavior and avoids paying TCP/TLS connection setup cost for every request.

Use `--no-keep-alive` when you intentionally want every request to use a fresh connection.

Examples:

```bash
# Default behavior: reuse connections
clank-cli http://localhost:8080 --keep-alive -n 100

# Disable connection reuse
clank-cli http://localhost:8080 --no-keep-alive -n 100
```

Use keep-alive when:

* You want realistic API client behavior.
* You are benchmarking steady-state request handling.
* You want to reduce connection setup overhead.

Use `--no-keep-alive` when:

* You want to benchmark cold connection behavior.
* You want to stress connection accept/close handling.
* You want to compare reused connections vs fresh connections.

`--keep-alive` is the default. `--no-keep-alive` disables connection reuse.

## Response Validation

Use response validation when you want to load test an endpoint and verify that each response still matches the expected contract. This is useful for checking status codes, response body content, and headers without switching to a separate `curl` command.

Validation failures are counted as errors in the final stats. They also increment the `validation_errors` field in JSON and CSV output, so you can distinguish response validation failures from connection errors, timeouts, and HTTP 5xx errors.

When validation fails, `clank-cli` still prints the selected summary output first, then prints validation failure details to stderr and exits with status code `1`.

Examples:

```bash
# Expect every response to return HTTP 200
clank-cli http://localhost:8080 --expect-status 200

# Allow multiple successful status codes
clank-cli http://localhost:8080 --expect-status 200,201,204

# Expect the response body to contain a case-sensitive pattern
clank-cli http://localhost:8080 --expect-body '"status":"ok"'

# Expect a response header
clank-cli http://localhost:8080 --expect-header "Content-Type: application/json"

# Combine status, body, and header validation
clank-cli http://localhost:8080 \
  --expect-status 200 \
  --expect-body "ok" \
  --expect-header "Content-Type: application/json"
```

Validation behavior:

| Flag                           | Behavior                                                                                               |
| ------------------------------ | ------------------------------------------------------------------------------------------------------ |
| `--expect-status <CODES>`      | Validates that the response status code is included in the comma-separated list.                       |
| `--expect-body <PATTERN>`      | Matches a case-sensitive pattern against the response body. Simple text works like substring matching. |
| `--expect-header <KEY: VALUE>` | Validates a response header. Header key matching is case-insensitive. Header value matching is exact.  |

Response validation runs after retry handling. For example, if a request returns `503` and then succeeds with `200` after retry, validation is applied to the final response.

## HTML Report

Use `--output html` to generate a self-contained browser report instead of printing the full summary to stdout.

HTML reports are useful when you want to share load test results with teammates or review charts visually after a run. The report includes summary cards, latency percentiles, latency histogram, status code breakdown, error distribution, response validation details, command metadata, and version information.

Examples:

```bash
# Generate an HTML report with an automatic file name
clank-cli http://localhost:8080 -n 1000 -c 20 --output html

# Write the HTML report to a specific path
clank-cli http://localhost:8080 -n 1000 -c 20 \
  --output html \
  --output-file reports/local-api-report.html

# Combine HTML report with validation, retry, rate limiting, and ramp-up
clank-cli http://localhost:8080 \
  --output html \
  --output-file reports/load-test.html \
  --expect-status 200 \
  --expect-body "ok" \
  --retry 3 \
  --rate-limit 100/s \
  --ramp-up 10s \
  --duration 30s
```

When `--output-file` is omitted, `clank-cli` generates a file name based on the target URL and current timestamp:

```text
<target>-report-<timestamp>.html
```

The generated report is self-contained. It embeds the report data, CSS, JavaScript, and Chart.js bundle directly in the HTML file, so it can be opened without an internet connection.

HTML report output prints the generated file path to stdout:

```text
HTML report written to http-localhost-8080-report-1700000000.html
```

## CLI Reference

| Flag | Short | Description | Default |
|------|-------|-------------|---------|
| `[URL]` / `--url` | | Target URL (required) | — |
| `-X, --method` | `-X` | HTTP method | GET |
| `-c, --concurrency` | `-c` | Concurrent workers | 10 |
| `--ramp-up <DURATION>` | | Ramp-up duration (`10s`, `1m`, `1h30m`) | disabled |
| `--ramp-up-step <N>` | | Workers added per ramp-up step, for example `5` | 1 |
| `--retry <N>` | | Retry failed requests up to N times | 0 |
| `--retry-delay <DURATION>` | | Delay between retry attempts (`100ms`, `1s`, `500ms`) | 0ms |
| `-r, --rate-limit` | `-r` | Limit request rate (`100/s`, `5000/m`, `10000/h`) | unlimited |
| `-n, --requests` | `-n` | Total requests to send | until Ctrl+C |
| `-d, --duration` | `-d` | Run duration (`5s`, `5m`, `1h30m`) | until Ctrl+C |
| `--body` | | Request body (inline) | — |
| `-B, --body-file` | `-B` | Request body from file | — |
| `-T, --content-type` | `-T` | Content-Type header | — |
| `-H, --header` | `-H` | Custom header (repeatable) | — |
| `--expect-status <CODES>` | | Validate response status code, comma-separated | — |
| `--expect-body <PATTERN>` | | Validate response body with a case-sensitive pattern | — |
| `--expect-header <KEY: VALUE>` | | Validate response header (repeatable) | — |
| `--timeout-secs` | | Request timeout in seconds | 10 |
| `-o, --output` | `-o` | Output format (`text`, `json`, `csv`, `html`) | text |
| `--output-file <PATH>` | | HTML report output path, only valid with `--output html` | auto-generated |
| `-f, --config` | `-f` | Config file path | `clank.yaml` |
| `--no-config` | | Skip config file | false |
| `-k, --insecure` | `-k` | Skip TLS verification | false |
| `--keep-alive` / `--no-keep-alive` | | Control HTTP connection reuse | keep-alive |
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
Validation Errors: 3
Retries:           12
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
Rate Limit:        100/s
Duration:          10.00s
────────────────────────────────
```

### JSON (`-o json`)

```json
{
  "total_requests": 1234,
  "validation_errors": 3,
  "retries": 12,
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
  "rate_limit": "100/s",
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
total_requests,validation_errors,retries,successful,errors,error_rate,avg_ms,p50_ms,p95_ms,p99_ms,p999_ms,throughput_rps,rate_limit,duration_secs,timeout,connection,http_4xx,http_5xx,http_other,other
1234,3,12,1200,34,2.8,45.2,42.0,78.0,156.7,230.1,123.4,100/s,10.00,12,8,14,0,0,0
```

### HTML (`-o html`)

HTML output writes a self-contained `.html` report file and prints the generated path to stdout.

```bash
clank-cli http://localhost:8080 -n 1000 -c 20 -o html
```

Example stdout:

```text
HTML report written to http-localhost-8080-report-1700000000.html
```

Use `--output-file` to choose the destination:

```bash
clank-cli http://localhost:8080 -n 1000 -c 20 \
  -o html \
  --output-file reports/load-test.html
```

The report includes:

* Summary cards for total requests, throughput, success rate, and errors
* Latency percentile table
* Latency histogram chart
* Status code breakdown chart
* Error distribution chart
* Response validation details
* Target URL, command, duration, version, and rate limit metadata
* Bundled Chart.js for offline viewing

## FAQ

### Why should I use ramp-up?

Use ramp-up when you want to avoid an artificial burst at the beginning of a load test. It helps simulate traffic that grows gradually, such as users joining over time.

Without ramp-up, all workers start immediately. That is useful for burst testing, but it can be less realistic for normal traffic simulation.

### How should I choose the ramp-up step size?

Use a smaller step size for smoother ramp-up and a larger step size for faster ramp-up.

For example:

- `--ramp-up-step 1` adds workers one by one.
- `--ramp-up-step 5` adds 5 workers per step.
- `--ramp-up-step 10` is useful for high-concurrency tests where one-by-one ramp-up would be too slow.

The step interval is based on:

```text
ramp_up_duration / ceil(concurrency / ramp_up_step)
```

### When should I enable retry?

Enable retry when you want to tolerate transient server-side failures during a load test.

Retry is best for HTTP 5xx, connection failures, and timeouts. It is not used for HTTP 4xx responses because 4xx usually means the request itself is invalid or unauthorized.

### When should I disable keep-alive?

Disable keep-alive when you want to benchmark cold connection behavior.

For most API load tests, keep-alive should remain enabled because that is closer to normal production client behavior.

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
- **HTML reports:** Self-contained template with bundled Chart.js
- **Config:** [serde_yaml](https://docs.rs/serde_yaml)

## License

[MIT](LICENSE.md) — Copyright (c) 2026 kodokbakar
