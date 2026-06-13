# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [0.2.0] - Unreleased

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
