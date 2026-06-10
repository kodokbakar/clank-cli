use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};
use clank_cli::config::{
    ClankConfig, DEFAULT_CONFIG_FILE, parse_duration, parse_header, validate_method,
};
use clank_cli::engine::{Engine, EngineConfig};
use clank_cli::stats::format_summary_with_color;
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
    let body = resolve_body(&cli, file_config.as_ref());
    let headers = resolve_headers(&cli, file_config.as_ref())?;
    let concurrency = resolve_concurrency(&cli, file_config.as_ref());
    let timeout_secs = resolve_timeout_secs(&cli, file_config.as_ref());
    let insecure = resolve_insecure(&cli, file_config.as_ref());

    if timeout_secs == 0 {
        bail!("timeout_secs must be greater than 0");
    }

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
    let output_color_enabled = color_enabled(cli.no_color);
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
        format_summary_with_color(&snapshot, output_color_enabled)
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

fn resolve_body(cli: &Cli, config: Option<&ClankConfig>) -> Option<String> {
    cli.body
        .clone()
        .or_else(|| config.and_then(|config| config.body.clone()))
}

fn resolve_headers(cli: &Cli, config: Option<&ClankConfig>) -> Result<Vec<(String, String)>> {
    if !cli.headers.is_empty() {
        return parse_headers(&cli.headers);
    }

    if let Some(config) = config {
        return parse_headers(&config.headers);
    }

    Ok(Vec::new())
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

fn parse_duration_arg(value: &str) -> Result<Duration, String> {
    parse_duration(value).map_err(|error| error.to_string())
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
            headers: Vec::new(),
            concurrency: None,
            requests: None,
            duration: None,
            timeout_secs: None,
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
            concurrency: 20,
            timeout_secs: 30,
            headers: vec!["Authorization: Bearer config".to_string()],
            insecure: true,
            output: None,
        }
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
    fn resolve_body_prefers_cli_over_config() {
        let mut cli = cli();
        cli.body = Some("from-cli".to_string());

        assert_eq!(
            resolve_body(&cli, Some(&config())),
            Some("from-cli".to_string())
        );
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
            vec![("Authorization".to_string(), "Bearer config".to_string())]
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
}
