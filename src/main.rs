use std::time::Duration;

use anyhow::{Result, bail};
use clank_cli::config::parse_duration;
use clank_cli::engine::{Engine, EngineConfig};
use clank_cli::stats::format_summary_with_color;
use clap::Parser;
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

    #[arg(short = 'X', long, default_value = "GET")]
    method: String,

    #[arg(long)]
    body: Option<String>,

    #[arg(short, long, default_value_t = 10)]
    concurrency: usize,

    #[arg(short = 'n', long)]
    requests: Option<usize>,

    #[arg(short, long, value_parser = parse_duration_arg)]
    duration: Option<Duration>,

    #[arg(long, default_value_t = 10)]
    timeout_secs: u64,

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

    if cli.stats_interval_ms == 0 {
        bail!("--stats-interval-ms must be greater than 0");
    }

    let url = resolve_url(&cli)?;

    let config = EngineConfig {
        url,
        method: cli.method,
        body: cli.body,
        concurrency: cli.concurrency,
        timeout: Duration::from_secs(cli.timeout_secs),
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
        println!("Running load test. Press Ctrl+C to stop.");
        engine.run().await?
    };

    println!(
        "{}",
        format_summary_with_color(&snapshot, output_color_enabled)
    );

    Ok(())
}

fn resolve_url(cli: &Cli) -> Result<String> {
    match (&cli.target_url, &cli.url) {
        (Some(_), Some(_)) => {
            bail!("provide target URL either as positional argument or --url, not both")
        }
        (Some(url), None) | (None, Some(url)) => Ok(url.clone()),
        (None, None) => bail!("target URL is required. Usage: clank-cli <URL> -c 10 -d 5s"),
    }
}

fn parse_duration_arg(value: &str) -> Result<Duration, String> {
    parse_duration(value).map_err(|error| error.to_string())
}

fn color_enabled(no_color: bool) -> bool {
    !no_color && std::env::var_os("NO_COLOR").is_none() && Term::stdout().is_term()
}
