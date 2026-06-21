use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clank_cli::config::{
    ClankConfig, DEFAULT_CONFIG_FILE, OutputFormat, RateLimitConfig, parse_duration, parse_header,
    parse_output_format, parse_rate_limit, validate_method,
};
use clank_cli::engine::{Engine, EngineConfig, RateLimiter, ValidationConfig};
use clank_cli::stats::format_summary_with_rate_limit_and_color_and_format;
use clap::{ArgAction, Parser};
use console::Term;

#[derive(Debug, Parser)]
#[command(
    name = "clank-cli",
    version,
    about = "HTTP load testing CLI tool built with Rust"
)]
struct Cli {
    #[arg(value_name = "URL")]
    target_url: Option<String>,

    #[arg(long, value_name = "URL")]
    url: Option<String>,

    #[arg(short = 'f', long, value_name = "FILE")]
    config: Option<PathBuf>,

    #[arg(long)]
    no_config: bool,

    #[arg(short = 'X', long, value_name = "METHOD")]
    method: Option<String>,

    #[arg(long)]
    body: Option<String>,

    #[arg(short = 'B', long = "body-file", value_name = "FILE")]
    body_file: Option<PathBuf>,

    #[arg(short = 'T', long = "content-type", value_name = "CONTENT_TYPE")]
    content_type: Option<String>,

    #[arg(short = 'H', long = "header", value_name = "KEY: VALUE", action = ArgAction::Append)]
    headers: Vec<String>,

    #[arg(long = "expect-status", value_name = "CODE")]
    expect_status: Option<String>,

    #[arg(long = "expect-body", value_name = "PATTERN")]
    expect_body: Option<String>,

    #[arg(long = "expect-header", value_name = "KEY: VALUE", action = ArgAction::Append)]
    expect_headers: Vec<String>,

    #[arg(short, long)]
    concurrency: Option<usize>,

    #[arg(short = 'r', long = "rate-limit", value_name = "RATE", value_parser = parse_rate_limit_arg)]
    rate_limit: Option<RateLimitConfig>,

    #[arg(short = 'n', long)]
    requests: Option<usize>,

    #[arg(short, long, value_parser = parse_duration_arg)]
    duration: Option<Duration>,

    #[arg(long = "ramp-up", value_name = "DURATION", value_parser = parse_ramp_up_arg)]
    ramp_up: Option<Duration>,

    #[arg(long = "ramp-up-step", value_name = "STEP", default_value_t = 1)]
    ramp_up_step: usize,

    #[arg(long = "retry", default_value_t = 0)]
    retry: usize,

    #[arg(
        long = "retry-delay",
        value_name = "DURATION",
        value_parser = parse_retry_delay_arg,
        default_value = "0ms"
    )]
    retry_delay: Duration,

    #[arg(long)]
    timeout_secs: Option<u64>,

    #[arg(short = 'o', long, value_name = "FORMAT", value_parser = parse_output_format_arg)]
    output: Option<OutputFormat>,

    #[arg(short = 'k', long)]
    insecure: bool,

    #[arg(long = "keep-alive", action = ArgAction::SetTrue, conflicts_with = "no_keep_alive")]
    keep_alive: bool,

    #[arg(long = "no-keep-alive", action = ArgAction::SetTrue, conflicts_with = "keep_alive")]
    no_keep_alive: bool,

    #[arg(short, long)]
    quiet: bool,

    #[arg(long, default_value_t = 1_000)]
    stats_interval_ms: u64,

    #[arg(long)]
    no_color: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.requests.is_some() && cli.duration.is_some() {
        bail!("use either --requests or --duration, not both");
    }

    if cli.config.is_some() && cli.no_config {
        bail!("use either --config or --no-config, not both");
    }

    if cli.stats_interval_ms == 0 {
        bail!("--stats-interval-ms must be greater than 0");
    }

    if cli.ramp_up_step == 0 {
        bail!("--ramp-up-step must be greater than 0");
    }

    let file_config = load_config(&cli)?;

    let url = resolve_url(&cli, file_config.as_ref())?;
    let method = resolve_method(&cli, file_config.as_ref())?;
    let body = resolve_body(&cli, file_config.as_ref())?;
    let headers = resolve_headers(&cli, file_config.as_ref())?;
    let validation = resolve_validation(&cli)?;
    let concurrency = resolve_concurrency(&cli, file_config.as_ref());
    let timeout_secs = resolve_timeout_secs(&cli, file_config.as_ref());
    let insecure = resolve_insecure(&cli, file_config.as_ref());
    let output_format = resolve_output_format(&cli, file_config.as_ref())?;
    let rate_limit = resolve_rate_limit(&cli, file_config.as_ref());
    let ramp_up = resolve_ramp_up(&cli);
    let ramp_up_step = resolve_ramp_up_step(&cli);
    let keep_alive = resolve_keep_alive(&cli);
    let retry = cli.retry;
    let retry_delay = cli.retry_delay;
    let rate_limiter = rate_limit
        .map(RateLimiter::from_config)
        .transpose()?
        .map(Arc::new);

    if timeout_secs == 0 {
        bail!("timeout_secs must be greater than 0");
    }

    warn_if_body_is_missing(&method, body.as_ref());

    let config = EngineConfig {
        url,
        method,
        body,
        headers,
        validation,
        concurrency,
        timeout: Duration::from_secs(timeout_secs),
        insecure,
        rate_limit,
        rate_limiter,
        ramp_up,
        ramp_up_step,
        keep_alive,
        retry,
        retry_delay,
    };

    let progress_enabled = !cli.quiet;
    let output_color_enabled = output_format == OutputFormat::Text && color_enabled(cli.no_color);
    let stats_interval = Duration::from_millis(cli.stats_interval_ms);

    let engine = Engine::new_with_progress_color_and_live_stats_interval(
        config,
        progress_enabled,
        output_color_enabled,
        stats_interval,
    )?;

    let snapshot = if let Some(total_requests) = cli.requests {
        engine.run_for_requests(total_requests).await?
    } else if let Some(duration) = cli.duration {
        engine.run_for_duration(duration).await?
    } else {
        eprintln!("Running load test. Press Ctrl+C to stop.");
        engine.run().await?
    };

    println!(
        "{}",
        format_summary_with_rate_limit_and_color_and_format(
            &snapshot,
            output_format,
            output_color_enabled,
            rate_limit.as_ref(),
        )
    );

    exit_if_validation_failed(&snapshot);

    Ok(())
}

fn exit_if_validation_failed(snapshot: &clank_cli::stats::StatsSnapshot) {
    if snapshot.validation_errors == 0 {
        return;
    }

    eprintln!("Validation failed:");

    for error in &snapshot.errors {
        eprintln!("- {error}");
    }

    std::process::exit(1);
}

fn load_config(cli: &Cli) -> Result<Option<ClankConfig>> {
    if cli.no_config {
        return Ok(None);
    }

    let path = cli
        .config
        .clone()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_FILE));

    ClankConfig::optional_from_file(path)
}

fn resolve_url(cli: &Cli, config: Option<&ClankConfig>) -> Result<String> {
    match (&cli.target_url, &cli.url) {
        (Some(_), Some(_)) => {
            bail!("provide target URL either as positional argument or --url, not both")
        }
        (Some(url), None) | (None, Some(url)) => Ok(url.clone()),
        (None, None) => match config {
            Some(config) => Ok(config.url.clone()),
            None => bail!("target URL is required. Usage: clank-cli <URL> -c 10 -d 5s"),
        },
    }
}

fn resolve_method(cli: &Cli, config: Option<&ClankConfig>) -> Result<String> {
    let method = cli
        .method
        .as_deref()
        .or_else(|| config.map(|config| config.method.as_str()))
        .unwrap_or("GET");

    validate_method(method)
}

fn resolve_body(cli: &Cli, config: Option<&ClankConfig>) -> Result<Option<String>> {
    if cli.body.is_some() && cli.body_file.is_some() {
        bail!("use either --body or --body-file, not both");
    }

    if let Some(body) = &cli.body {
        return Ok(Some(body.clone()));
    }

    if let Some(body_file) = &cli.body_file {
        return read_body_file(body_file).map(Some);
    }

    if let Some(config) = config {
        if let Some(body_file) = &config.body_file {
            return read_body_file(body_file).map(Some);
        }

        if let Some(body) = &config.body {
            return Ok(Some(body.clone()));
        }
    }

    Ok(None)
}

fn resolve_headers(cli: &Cli, config: Option<&ClankConfig>) -> Result<Vec<(String, String)>> {
    let mut headers = if !cli.headers.is_empty() {
        parse_headers(&cli.headers)?
    } else if let Some(config) = config {
        parse_headers(&config.headers)?
    } else {
        Vec::new()
    };

    if let Some(content_type) = resolve_content_type(cli, config) {
        apply_content_type_header(&mut headers, content_type)?;
    }

    Ok(headers)
}

fn resolve_validation(cli: &Cli) -> Result<ValidationConfig> {
    Ok(ValidationConfig {
        expect_status: cli
            .expect_status
            .as_deref()
            .map(parse_expect_status)
            .transpose()?,
        expect_body: cli.expect_body.clone(),
        expect_headers: if cli.expect_headers.is_empty() {
            None
        } else {
            Some(parse_headers(&cli.expect_headers)?)
        },
    })
}

fn parse_expect_status(input: &str) -> Result<Vec<u16>> {
    let mut statuses = Vec::new();

    for raw_status in input.split(',') {
        let raw_status = raw_status.trim();

        if raw_status.is_empty() {
            bail!("expected status code cannot be empty");
        }

        let status: u16 = raw_status
            .parse()
            .with_context(|| format!("invalid status code: {raw_status}"))?;

        if !(100..=599).contains(&status) {
            bail!("status code must be between 100 and 599: {status}");
        }

        statuses.push(status);
    }

    Ok(statuses)
}

fn resolve_content_type<'a>(cli: &'a Cli, config: Option<&'a ClankConfig>) -> Option<&'a str> {
    if let Some(content_type) = &cli.content_type {
        return Some(content_type.as_str());
    }

    if headers_contain_key(&cli.headers, "content-type") {
        return None;
    }

    config.and_then(|config| config.content_type.as_deref())
}

fn headers_contain_key(headers: &[String], expected_key: &str) -> bool {
    headers.iter().any(|header| match header.split_once(':') {
        Some((key, _)) => key.trim().eq_ignore_ascii_case(expected_key),
        None => false,
    })
}

fn apply_content_type_header(
    headers: &mut Vec<(String, String)>,
    content_type: &str,
) -> Result<()> {
    let content_type = content_type.trim();

    if content_type.is_empty() {
        bail!("content type cannot be empty");
    }

    headers.retain(|(key, _)| !key.eq_ignore_ascii_case("content-type"));
    headers.push(("Content-Type".to_string(), content_type.to_string()));

    Ok(())
}

fn read_body_file(path: impl AsRef<Path>) -> Result<String> {
    let path = path.as_ref();

    let bytes =
        fs::read(path).with_context(|| format!("failed to read body file: {}", path.display()))?;

    String::from_utf8(bytes)
        .with_context(|| format!("body file is not valid UTF-8: {}", path.display()))
}

fn warn_if_body_is_missing(method: &str, body: Option<&String>) {
    if body.is_some() {
        return;
    }

    match method {
        "POST" | "PUT" | "PATCH" => {
            eprintln!("Warning: {method} request has no body");
        }
        _ => {}
    }
}

fn resolve_concurrency(cli: &Cli, config: Option<&ClankConfig>) -> usize {
    cli.concurrency
        .or_else(|| config.map(|config| config.concurrency))
        .unwrap_or(10)
}

fn resolve_timeout_secs(cli: &Cli, config: Option<&ClankConfig>) -> u64 {
    cli.timeout_secs
        .or_else(|| config.map(|config| config.timeout_secs))
        .unwrap_or(10)
}

fn resolve_insecure(cli: &Cli, config: Option<&ClankConfig>) -> bool {
    if cli.insecure {
        true
    } else {
        config.map(|config| config.insecure).unwrap_or(false)
    }
}

fn resolve_output_format(cli: &Cli, config: Option<&ClankConfig>) -> Result<OutputFormat> {
    if let Some(output) = cli.output {
        return Ok(output);
    }

    if let Some(config) = config
        && let Some(output) = &config.output
    {
        return parse_output_format(output);
    }

    Ok(OutputFormat::Text)
}

fn resolve_rate_limit(cli: &Cli, config: Option<&ClankConfig>) -> Option<RateLimitConfig> {
    cli.rate_limit
        .or_else(|| config.and_then(|config| config.rate_limit))
}

fn resolve_ramp_up(cli: &Cli) -> Option<Duration> {
    cli.ramp_up.filter(|duration| !duration.is_zero())
}

fn resolve_ramp_up_step(cli: &Cli) -> usize {
    cli.ramp_up_step
}

fn resolve_keep_alive(cli: &Cli) -> bool {
    cli.keep_alive || !cli.no_keep_alive
}

fn parse_duration_arg(value: &str) -> Result<Duration, String> {
    parse_duration(value).map_err(|error| error.to_string())
}

fn parse_ramp_up_arg(value: &str) -> Result<Duration, String> {
    let value = value.trim();

    if matches!(value, "0" | "0s" | "0m" | "0h") {
        return Ok(Duration::ZERO);
    }

    parse_duration(value).map_err(|error| error.to_string())
}

fn parse_retry_delay_arg(value: &str) -> Result<Duration, String> {
    let value = value.trim();

    if matches!(value, "0" | "0ms" | "0s" | "0m" | "0h") {
        return Ok(Duration::ZERO);
    }

    parse_duration(value).map_err(|error| error.to_string())
}

fn parse_output_format_arg(value: &str) -> Result<OutputFormat, String> {
    parse_output_format(value).map_err(|error| error.to_string())
}

fn parse_rate_limit_arg(value: &str) -> Result<RateLimitConfig, String> {
    parse_rate_limit(value).map_err(|error| error.to_string())
}

fn parse_headers(headers: &[String]) -> Result<Vec<(String, String)>> {
    headers.iter().map(|header| parse_header(header)).collect()
}

fn color_enabled(no_color: bool) -> bool {
    !no_color && std::env::var_os("NO_COLOR").is_none() && Term::stdout().is_term()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli() -> Cli {
        Cli {
            target_url: None,
            url: None,
            config: None,
            no_config: false,
            method: None,
            body: None,
            body_file: None,
            content_type: None,
            headers: Vec::new(),
            expect_status: None,
            expect_body: None,
            expect_headers: Vec::new(),
            concurrency: None,
            rate_limit: None,
            requests: None,
            duration: None,
            ramp_up: None,
            ramp_up_step: 1,
            retry: 0,
            retry_delay: Duration::ZERO,
            timeout_secs: None,
            output: None,
            insecure: false,
            keep_alive: false,
            no_keep_alive: false,
            quiet: false,
            stats_interval_ms: 1_000,
            no_color: false,
        }
    }

    fn config() -> ClankConfig {
        ClankConfig {
            url: "http://config.test".to_string(),
            method: "POST".to_string(),
            body: Some("from-config".to_string()),
            body_file: None,
            content_type: Some("application/json".to_string()),
            concurrency: 20,
            timeout_secs: 30,
            headers: vec!["Authorization: Bearer config".to_string()],
            insecure: true,
            output: Some("json".to_string()),
            rate_limit: Some(RateLimitConfig {
                rate: 100,
                period: clank_cli::config::RatePeriod::Second,
            }),
        }
    }

    #[test]
    fn resolve_output_format_prefers_cli_over_config() -> Result<()> {
        let mut cli = cli();
        cli.output = Some(OutputFormat::Csv);

        assert_eq!(
            resolve_output_format(&cli, Some(&config()))?,
            OutputFormat::Csv
        );

        Ok(())
    }

    #[test]
    fn resolve_output_format_uses_config_when_cli_missing() -> Result<()> {
        assert_eq!(
            resolve_output_format(&cli(), Some(&config()))?,
            OutputFormat::Json
        );

        Ok(())
    }

    #[test]
    fn resolve_output_format_uses_text_default() -> Result<()> {
        assert_eq!(resolve_output_format(&cli(), None)?, OutputFormat::Text);

        Ok(())
    }

    #[test]
    fn resolve_output_format_rejects_invalid_config_value() {
        let mut config = config();
        config.output = Some("xml".to_string());

        assert!(resolve_output_format(&cli(), Some(&config)).is_err());
    }

    #[test]
    fn resolve_url_prefers_positional_cli_url_over_config() -> Result<()> {
        let mut cli = cli();
        cli.target_url = Some("http://cli.test".to_string());

        assert_eq!(resolve_url(&cli, Some(&config()))?, "http://cli.test");

        Ok(())
    }

    #[test]
    fn resolve_url_prefers_named_cli_url_over_config() -> Result<()> {
        let mut cli = cli();
        cli.url = Some("http://cli.test".to_string());

        assert_eq!(resolve_url(&cli, Some(&config()))?, "http://cli.test");

        Ok(())
    }

    #[test]
    fn resolve_url_uses_config_when_cli_url_missing() -> Result<()> {
        assert_eq!(resolve_url(&cli(), Some(&config()))?, "http://config.test");

        Ok(())
    }

    #[test]
    fn resolve_url_rejects_duplicate_cli_urls() {
        let mut cli = cli();
        cli.target_url = Some("http://positional.test".to_string());
        cli.url = Some("http://named.test".to_string());

        assert!(resolve_url(&cli, Some(&config())).is_err());
    }

    #[test]
    fn resolve_method_prefers_cli_over_config() -> Result<()> {
        let mut cli = cli();
        cli.method = Some("put".to_string());

        assert_eq!(resolve_method(&cli, Some(&config()))?, "PUT");

        Ok(())
    }

    #[test]
    fn resolve_method_uses_config_when_cli_missing() -> Result<()> {
        assert_eq!(resolve_method(&cli(), Some(&config()))?, "POST");

        Ok(())
    }

    #[test]
    fn resolve_method_uses_default_when_cli_and_config_missing() -> Result<()> {
        assert_eq!(resolve_method(&cli(), None)?, "GET");

        Ok(())
    }

    #[test]
    fn resolve_body_prefers_cli_over_config() -> Result<()> {
        let mut cli = cli();
        cli.body = Some("from-cli".to_string());

        assert_eq!(
            resolve_body(&cli, Some(&config()))?,
            Some("from-cli".to_string())
        );

        Ok(())
    }

    #[test]
    fn resolve_headers_prefers_cli_headers_but_keeps_config_content_type() -> Result<()> {
        let mut cli = cli();
        cli.headers = vec!["Authorization: Bearer cli".to_string()];

        let headers = resolve_headers(&cli, Some(&config()))?;

        assert_eq!(
            headers,
            vec![
                ("Authorization".to_string(), "Bearer cli".to_string()),
                ("Content-Type".to_string(), "application/json".to_string()),
            ]
        );

        Ok(())
    }

    #[test]
    fn resolve_headers_uses_config_when_cli_missing() -> Result<()> {
        let headers = resolve_headers(&cli(), Some(&config()))?;

        assert_eq!(
            headers,
            vec![
                ("Authorization".to_string(), "Bearer config".to_string()),
                ("Content-Type".to_string(), "application/json".to_string()),
            ]
        );

        Ok(())
    }

    #[test]
    fn resolve_concurrency_prefers_cli_over_config() {
        let mut cli = cli();
        cli.concurrency = Some(50);

        assert_eq!(resolve_concurrency(&cli, Some(&config())), 50);
    }

    #[test]
    fn resolve_timeout_prefers_cli_over_config() {
        let mut cli = cli();
        cli.timeout_secs = Some(99);

        assert_eq!(resolve_timeout_secs(&cli, Some(&config())), 99);
    }

    #[test]
    fn resolve_insecure_uses_config_when_cli_missing() {
        assert!(resolve_insecure(&cli(), Some(&config())));
    }

    #[test]
    fn resolve_insecure_prefers_cli_true() {
        let mut cli = cli();
        cli.insecure = true;

        assert!(resolve_insecure(&cli, None));
    }

    #[test]
    fn resolve_headers_cli_replaces_config_headers_but_keeps_config_content_type() -> Result<()> {
        let mut cli = cli();

        cli.headers = vec![
            "Authorization: Bearer cli".to_string(),
            "X-Source: cli".to_string(),
        ];

        let headers = resolve_headers(&cli, Some(&config()))?;

        assert_eq!(
            headers,
            vec![
                ("Authorization".to_string(), "Bearer cli".to_string()),
                ("X-Source".to_string(), "cli".to_string()),
                ("Content-Type".to_string(), "application/json".to_string()),
            ]
        );

        Ok(())
    }

    fn unique_body_file_path(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after UNIX epoch")
            .as_nanos();

        std::env::temp_dir().join(format!("clank-cli-{name}-{nanos}.json"))
    }

    #[test]
    fn resolve_body_uses_body_file_when_cli_body_missing() -> Result<()> {
        let path = unique_body_file_path("body-file");
        fs::write(&path, r#"{"name":"file"}"#)?;

        let mut cli = cli();
        cli.body_file = Some(path.clone());

        assert_eq!(
            resolve_body(&cli, Some(&config()))?,
            Some(r#"{"name":"file"}"#.to_string())
        );

        fs::remove_file(path)?;

        Ok(())
    }

    #[test]
    fn resolve_body_rejects_missing_body_file() {
        let mut cli = cli();
        cli.body_file = Some(PathBuf::from("missing-body-file.json"));

        let error = resolve_body(&cli, None).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("failed to read body file: missing-body-file.json")
        );
    }

    #[test]
    fn resolve_body_rejects_non_utf8_body_file() -> Result<()> {
        let path = unique_body_file_path("non-utf8-body-file");

        fs::write(&path, [0xff, 0xfe, 0xfd])?;

        let mut cli = cli();
        cli.body_file = Some(path.clone());

        let error = resolve_body(&cli, None).unwrap_err();

        assert!(error.to_string().contains("body file is not valid UTF-8"));

        fs::remove_file(path)?;

        Ok(())
    }

    #[test]
    fn resolve_body_rejects_body_and_body_file_together() -> Result<()> {
        let mut cli = cli();
        cli.body = Some("raw-body".to_string());
        cli.body_file = Some(PathBuf::from("request.json"));

        let error = resolve_body(&cli, None).unwrap_err();

        assert_eq!(
            error.to_string(),
            "use either --body or --body-file, not both"
        );

        Ok(())
    }

    #[test]
    fn resolve_body_uses_config_body_file_when_present() -> Result<()> {
        let path = unique_body_file_path("config-body-file");
        fs::write(&path, r#"{"name":"config-file"}"#)?;

        let cli = cli();
        let mut config = config();
        config.body = None;
        config.body_file = Some(path.clone());

        assert_eq!(
            resolve_body(&cli, Some(&config))?,
            Some(r#"{"name":"config-file"}"#.to_string())
        );

        fs::remove_file(path)?;

        Ok(())
    }

    #[test]
    fn resolve_headers_applies_cli_content_type() -> Result<()> {
        let mut cli = cli();
        cli.content_type = Some("application/json".to_string());

        let headers = resolve_headers(&cli, None)?;

        assert_eq!(
            headers,
            vec![("Content-Type".to_string(), "application/json".to_string())]
        );

        Ok(())
    }

    #[test]
    fn resolve_headers_content_type_overrides_duplicate_header() -> Result<()> {
        let mut cli = cli();
        cli.headers = vec!["Content-Type: text/plain".to_string()];
        cli.content_type = Some("application/json".to_string());

        let headers = resolve_headers(&cli, None)?;

        assert_eq!(
            headers,
            vec![("Content-Type".to_string(), "application/json".to_string())]
        );

        Ok(())
    }

    #[test]
    fn resolve_headers_preserves_explicit_content_type_header_without_flag() -> Result<()> {
        let mut cli = cli();
        cli.headers = vec!["Content-Type: text/plain".to_string()];

        let headers = resolve_headers(&cli, Some(&config()))?;

        assert_eq!(
            headers,
            vec![("Content-Type".to_string(), "text/plain".to_string())]
        );

        Ok(())
    }

    #[test]
    fn resolve_headers_cli_non_content_type_header_keeps_config_content_type() -> Result<()> {
        let mut cli = cli();
        cli.headers = vec!["Accept: application/json".to_string()];

        let headers = resolve_headers(&cli, Some(&config()))?;

        assert_eq!(
            headers,
            vec![
                ("Accept".to_string(), "application/json".to_string()),
                ("Content-Type".to_string(), "application/json".to_string()),
            ]
        );

        Ok(())
    }

    #[test]
    fn resolve_headers_rejects_empty_content_type() {
        let mut cli = cli();
        cli.content_type = Some(" ".to_string());

        assert!(resolve_headers(&cli, None).is_err());
    }

    #[test]
    fn resolve_rate_limit_prefers_cli_over_config() {
        let mut cli = cli();
        cli.rate_limit = Some(RateLimitConfig {
            rate: 10,
            period: clank_cli::config::RatePeriod::Second,
        });

        assert_eq!(
            resolve_rate_limit(&cli, Some(&config())),
            Some(RateLimitConfig {
                rate: 10,
                period: clank_cli::config::RatePeriod::Second,
            })
        );
    }

    #[test]
    fn resolve_rate_limit_uses_config_when_cli_missing() {
        assert_eq!(
            resolve_rate_limit(&cli(), Some(&config())),
            Some(RateLimitConfig {
                rate: 100,
                period: clank_cli::config::RatePeriod::Second,
            })
        );
    }

    #[test]
    fn resolve_rate_limit_uses_none_when_cli_and_config_missing() {
        assert_eq!(resolve_rate_limit(&cli(), None), None);
    }

    #[test]
    fn parse_rate_limit_arg_accepts_supported_format() -> Result<()> {
        assert_eq!(
            parse_rate_limit_arg("5000/m").map_err(anyhow::Error::msg)?,
            RateLimitConfig {
                rate: 5000,
                period: clank_cli::config::RatePeriod::Minute,
            }
        );

        Ok(())
    }

    #[test]
    fn parse_rate_limit_arg_rejects_invalid_format() {
        assert!(parse_rate_limit_arg("10/d").is_err());
    }

    #[test]
    fn resolve_ramp_up_uses_none_when_cli_missing() {
        assert_eq!(resolve_ramp_up(&cli()), None);
    }

    #[test]
    fn resolve_ramp_up_uses_cli_value() {
        let mut cli = cli();
        cli.ramp_up = Some(Duration::from_secs(10));

        assert_eq!(resolve_ramp_up(&cli), Some(Duration::from_secs(10)));
    }

    #[test]
    fn resolve_ramp_up_treats_zero_as_none() {
        let mut cli = cli();
        cli.ramp_up = Some(Duration::ZERO);

        assert_eq!(resolve_ramp_up(&cli), None);
    }

    #[test]
    fn resolve_ramp_up_step_defaults_to_one() {
        assert_eq!(resolve_ramp_up_step(&cli()), 1);
    }

    #[test]
    fn resolve_ramp_up_step_uses_cli_value() {
        let mut cli = cli();
        cli.ramp_up_step = 5;

        assert_eq!(resolve_ramp_up_step(&cli), 5);
    }

    #[test]
    fn resolve_keep_alive_defaults_to_true() {
        assert!(resolve_keep_alive(&cli()));
    }

    #[test]
    fn resolve_keep_alive_uses_explicit_keep_alive() {
        let mut cli = cli();
        cli.keep_alive = true;

        assert!(resolve_keep_alive(&cli));
    }

    #[test]
    fn resolve_keep_alive_uses_no_keep_alive() {
        let mut cli = cli();
        cli.no_keep_alive = true;

        assert!(!resolve_keep_alive(&cli));
    }

    #[test]
    fn parse_retry_delay_arg_accepts_milliseconds() -> Result<()> {
        assert_eq!(
            parse_retry_delay_arg("100ms").map_err(anyhow::Error::msg)?,
            Duration::from_millis(100)
        );

        assert_eq!(
            parse_retry_delay_arg("500ms").map_err(anyhow::Error::msg)?,
            Duration::from_millis(500)
        );

        Ok(())
    }

    #[test]
    fn parse_retry_delay_arg_accepts_zero() -> Result<()> {
        assert_eq!(
            parse_retry_delay_arg("0ms").map_err(anyhow::Error::msg)?,
            Duration::ZERO
        );

        assert_eq!(
            parse_retry_delay_arg("0").map_err(anyhow::Error::msg)?,
            Duration::ZERO
        );

        Ok(())
    }

    #[test]
    fn parse_retry_delay_arg_rejects_invalid_duration() {
        assert!(parse_retry_delay_arg("10d").is_err());
    }

    #[test]
    fn parse_ramp_up_arg_accepts_regular_duration() -> Result<()> {
        assert_eq!(
            parse_ramp_up_arg("10s").map_err(anyhow::Error::msg)?,
            Duration::from_secs(10)
        );

        Ok(())
    }

    #[test]
    fn parse_ramp_up_arg_accepts_zero_as_disabled() -> Result<()> {
        assert_eq!(
            parse_ramp_up_arg("0s").map_err(anyhow::Error::msg)?,
            Duration::ZERO
        );

        assert_eq!(
            parse_ramp_up_arg("0").map_err(anyhow::Error::msg)?,
            Duration::ZERO
        );

        Ok(())
    }

    #[test]
    fn parse_ramp_up_arg_rejects_invalid_duration() {
        assert!(parse_ramp_up_arg("10d").is_err());
    }

    #[test]
    fn parse_expect_status_accepts_single_status() -> Result<()> {
        assert_eq!(parse_expect_status("200")?, vec![200]);

        Ok(())
    }

    #[test]
    fn parse_expect_status_accepts_multiple_statuses() -> Result<()> {
        assert_eq!(parse_expect_status("200,201,204")?, vec![200, 201, 204]);

        Ok(())
    }

    #[test]
    fn parse_expect_status_rejects_non_numeric_status() {
        assert!(parse_expect_status("ok").is_err());
    }

    #[test]
    fn parse_expect_status_rejects_out_of_range_status() {
        assert!(parse_expect_status("99").is_err());
        assert!(parse_expect_status("600").is_err());
    }

    #[test]
    fn resolve_validation_builds_validation_config() -> Result<()> {
        let mut cli = cli();
        cli.expect_status = Some("200,201".to_string());
        cli.expect_body = Some(r#""status":"ok""#.to_string());
        cli.expect_headers = vec!["Content-Type: application/json".to_string()];

        let validation = resolve_validation(&cli)?;

        assert_eq!(validation.expect_status, Some(vec![200, 201]));
        assert_eq!(validation.expect_body, Some(r#""status":"ok""#.to_string()));
        assert_eq!(
            validation.expect_headers,
            Some(vec![(
                "Content-Type".to_string(),
                "application/json".to_string()
            )])
        );

        Ok(())
    }
}
