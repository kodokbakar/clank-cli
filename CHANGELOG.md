# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [0.2.4] - 2026-06-21

### Added

- Integration test for invalid regex handling in `--expect-body`.
- Integration tests for `--expect-header` value mismatch, missing header, and multiple matching headers.

### Changed

- Validation failures now exit with a non-zero status code after printing the summary.
- Validation failure details are now printed to stderr.

### Removed

- Unused `ErrorCategory::ValidationFailure` dead code.

## [0.2.3] - 2026-06-21

### Added

- Response status validation with `--expect-status <CODES>`.
- Response body validation with `--expect-body <PATTERN>`.
- Response header validation with `--expect-header <KEY: VALUE>`.
- `validation_errors` field in stats output.
- Integration tests for response validation status, body, header, combined validation, and validation with retry.
- README documentation for response validation.

## [0.2.2] - 2026-06-20

### Added

- Retry support with `--retry <N>` for retrying transient request failures.
- Retry delay support with `--retry-delay <DURATION>` for controlling delay between retry attempts.
- Keep-alive control with `--keep-alive` and `--no-keep-alive` for HTTP connection reuse behavior.
- Retry stats in plain text, JSON, CSV, and live terminal output.
- Integration tests for retry behavior, retry delay, retry exhaustion, 4xx no-retry behavior, keep-alive behavior, and retry with rate limiting.
- README documentation for retry and keep-alive usage.
- crates.io package metadata for `cargo install clank-cli` distribution.

## [0.2.1] - 2026-06-16

### Added

- Ramp-up support with `--ramp-up <DURATION>` for gradual concurrency increase.
- Ramp-up step control with `--ramp-up-step <N>`.
- Live stats display for current and target workers during ramp-up.
- Integration tests for ramp-up behavior, step intervals, final worker count, short ramp-up duration, and backward compatibility without ramp-up.
- README documentation for ramp-up usage, step interval calculation, examples, and FAQ.

## [0.2.0] - 2026-06-14

### Added

- Rate limiting with `--rate-limit` / `-r`.
- Rate limit config support through `clank.yaml`.
- Rate limit formats: `N/s`, `N/m`, and `N/h`.
- Live stats display for active rate limit and actual throughput.
- Rate limit field in text, JSON, and CSV output summaries.
- Documentation and examples for rate limiting.

### Changed

- Output summary now includes `Rate Limit: unlimited` when no rate limit is active.

## [0.1.0] - 2026-06-10

### Added

- Concurrent HTTP load testing with configurable workers.
- Request-based test mode with `-n, --requests`.
- Duration-based test mode with `-d, --duration`.
- Continuous test mode that runs until `Ctrl+C`.
- Progress bar with live request progress.
- Live stats display for RPS, latency, success, and error counts.
- ETA estimation for request-based and duration-based runs.
- Color-coded terminal output.
- Graceful shutdown with `Ctrl+C`.
- Double `Ctrl+C` force quit behavior during shutdown.
- Multiple HTTP methods: `GET`, `POST`, `PUT`, `DELETE`, `PATCH`, `HEAD`, and `OPTIONS`.
- Custom headers with repeatable `-H, --header`.
- Raw request body support with `--body`.
- Request body file support with `-B, --body-file`.
- Dedicated `Content-Type` support with `-T, --content-type`.
- YAML config file support with `clank.yaml`.
- Config override support through CLI arguments.
- Multiple output formats: text, JSON, and CSV.
- Request timeout configuration with `--timeout-secs`.
- Insecure TLS mode with `-k, --insecure`.
- Quiet mode with `-q, --quiet`.
- Color disabling with `--no-color`.
- Live stats interval configuration with `--stats-interval-ms`.
- Error categorization for timeout, connection, HTTP 4xx, HTTP 5xx, HTTP other, and other errors.
- Latency summary with average, p50, p95, p99, and p999 percentiles.
- Cross-platform build scripts for Linux, macOS, and Windows.
- Release packaging scripts with `.tar.gz` and `.zip` artifacts.
- GitHub Actions CI pipeline for formatting, tests, and clippy.
- GitHub Actions release pipeline for tagged releases.
- Release assets with SHA256 checksums.
- Homebrew tap formula for macOS Intel and Apple Silicon.
- README documentation for installation, examples, CLI reference, config, outputs, build, and tech stack.
