use std::time::Duration;

use anyhow::Result;
use clank_cli::engine::{Engine, EngineConfig};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "clank-cli",
    version,
    about = "HTTP load testing CLI tool built with Rust"
)]
struct Cli {
    /// Target URL for the load test
    #[arg(short, long)]
    url: Option<String>,

    /// HTTP method to use
    #[arg(short = 'X', long, default_value = "GET")]
    method: String,

    /// Optional request body for POST requests
    #[arg(long)]
    body: Option<String>,

    /// Number of concurrent workers
    #[arg(short, long, default_value_t = 1)]
    concurrency: usize,

    /// Total requests to send. If omitted, runs until Ctrl+C.
    #[arg(short = 'n', long)]
    requests: Option<usize>,

    /// Request timeout in seconds
    #[arg(long, default_value_t = 10)]
    timeout_secs: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let Some(url) = cli.url else {
        println!("clank-cli scaffold is ready");
        println!("Run with: clank-cli --url <URL> --concurrency 10");
        return Ok(());
    };

    let config = EngineConfig {
        url,
        method: cli.method,
        body: cli.body,
        concurrency: cli.concurrency,
        timeout: Duration::from_secs(cli.timeout_secs),
    };

    let engine = Engine::new(config)?;

    let snapshot = match cli.requests {
        Some(total_requests) => engine.run_for_requests(total_requests).await?,
        None => {
            println!("Running load test. Press Ctrl+C to stop.");
            engine.run().await?
        }
    };

    println!("Total requests: {}", snapshot.total_requests);
    println!("Total errors: {}", snapshot.total_errors);
    println!("Status codes: {:?}", snapshot.status_codes);

    Ok(())
}
