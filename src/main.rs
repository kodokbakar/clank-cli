use anyhow::Result;
use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "crank-cli",
    version,
    about = "HTTP load testing CLI tool built with Rust"
)]
struct Cli {
    /// Target URL for the load test
    #[arg(short, long)]
    url: Option<String>,

    /// Number of concurrent requests
    #[arg(short, long, default_value_t = 1)]
    concurrency: u32,

    /// Total number of requests to send
    #[arg(short, long, default_value_t = 1)]
    requests: u32,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _cli = Cli::parse();

    println!("crank-cli scaffold is ready");

    Ok(())
}
