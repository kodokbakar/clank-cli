use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clank_cli::config::{
    ClankConfig, DEFAULT_CONFIG_FILE, OutputFormat, parse_duration, parse_header,
    parse_output_format, validate_method,
};
use clank_cli::engine::{Engine, EngineConfig};
use clank_cli::stats::format_summary_with_color_and_format;
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

    #[arg(short, long)]
    concurrency: Option<usize>,

    #[arg(short = 'n', long)]
    requests: Option<usize>,

    #[arg(short, long, value_parser = parse_duration_arg)]
    duration: Option<Duration>,

    #[arg(long)]
    timeout_secs: Option<u64>,

    #[arg(short = 'o', long, value_name = "FORMAT", value_parser = parse_output_format_arg)]
    output: Option<OutputFormat>,

    #[arg(short = 'k', long)]
    insecure: bool,

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

    let file_config = load_config(&cli)?;

    let url = resolve_url(&cli, file_config.as_ref())?;
    let method = resolve_method(&cli, file_config.as_ref())?;
    let body = resolve_body(&cli, file_config.as_ref())?;
    let headers = resolve_headers(&cli, file_config.as_ref())?;
    let concurrency = resolve_concurrency(&cli, file_config.as_ref());
    let timeout_secs = resolve_timeout_secs(&cli, file_config.as_ref());
    let insecure = resolve_insecure(&cli, file_config.as_ref());
    let output_format = resolve_output_format(&cli, file_config.as_ref())?;

    if timeout_secs == 0 {
        bail!("timeout_secs must be greater than 0");
    }

    warn_if_body_is_missing(&method, body.as_ref());

    let config = EngineConfig {
        url,
        method,
        body,
        headers,
        concurrency,
        timeout: Duration::from_secs(timeout_secs),
        insecure,
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
        format_summary_with_color_and_format(&snapshot, output_format, output_color_enabled)
    );

    Ok(())
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

fn resolve_content_type<'a>(cli: &'a Cli, config: Option<&'a ClankConfig>) -> Option<&'a str> {
    if let Some(content_type) = &cli.content_type {
        return Some(content_type.as_str());
    }

    if !cli.headers.is_empty() {
        return None;
    }

    config.and_then(|config| config.content_type.as_deref())
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

    if let Some(config) = config {
        if let Some(output) = &config.output {
            return parse_output_format(output);
        }
    }

    Ok(OutputFormat::Text)
}

fn parse_duration_arg(value: &str) -> Result<Duration, String> {
    parse_duration(value).map_err(|error| error.to_string())
}

fn parse_output_format_arg(value: &str) -> Result<OutputFormat, String> {
    parse_output_format(value).map_err(|error| error.to_string())
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
            concurrency: None,
            requests: None,
            duration: None,
            timeout_secs: None,
            output: None,
            insecure: false,
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
    fn resolve_headers_prefers_cli_over_config() -> Result<()> {
        let mut cli = cli();
        cli.headers = vec!["Authorization: Bearer cli".to_string()];

        let headers = resolve_headers(&cli, Some(&config()))?;

        assert_eq!(
            headers,
            vec![("Authorization".to_string(), "Bearer cli".to_string())]
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
    fn resolve_headers_cli_replaces_config_headers() -> Result<()> {
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
    fn resolve_headers_rejects_empty_content_type() {
        let mut cli = cli();
        cli.content_type = Some(" ".to_string());

        assert!(resolve_headers(&cli, None).is_err());
    }
}
