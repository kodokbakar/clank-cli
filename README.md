# clank-cli

HTTP load testing CLI tool built with Rust.

## Install

```bash
cargo install clank-cli
```

## Usage

```bash
# Basic: 10 concurrent requests, stops at Ctrl+C
clank-cli https://example.com

# 50 requests with 20 concurrent workers
clank-cli https://example.com -n 50 -c 20

# Run for 30 seconds
clank-cli https://example.com -d 30s

# POST request with body
clank-cli https://example.com -X POST --body '{"key":"value"}'
```

## CLI Options

| Flag | Description | Default |
|------|-------------|---------|
| `<URL>` / `--url` | Target URL (required) | — |
| `-c, --concurrency` | Concurrent workers | 10 |
| `-n, --requests` | Total requests to send | until Ctrl+C |
| `-d, --duration` | Run duration (`5s`, `5m`, `1h30m`) | until Ctrl+C |
| `-X, --method` | HTTP method | GET |
| `--body` | Request body | — |
| `--timeout-secs` | Request timeout in seconds | 10 |

## Output

```
Results:
────────────────────────────────
Total Requests:    1,234
Successful:        1,200 (97.2%)
Errors:            34 (2.8%)
  Timeout:         12
  Connection:      8
  HTTP Error:      14
────────────────────────────────
Latency (avg):     45.2ms
Throughput:        123.4 req/s
Duration:          10.00s
────────────────────────────────
```

## Tech Stack

- **Runtime:** Tokio
- **HTTP:** Reqwest (rustls-tls)
- **CLI:** Clap (derive)
- **Stats:** HdrHistogram (planned)

## License

[MIT](LICENSE.md)
