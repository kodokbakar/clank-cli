use std::io;
use std::process::Command;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

struct TestServer {
    url: String,
    requests: Arc<AtomicUsize>,
    connections: Arc<AtomicUsize>,
    handle: JoinHandle<()>,
}

impl TestServer {
    fn url(&self) -> &str {
        &self.url
    }

    fn request_count(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }

    fn connection_count(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn start_status_server(
    statuses: Vec<u16>,
    keep_alive: bool,
    response_delay: Duration,
) -> Result<TestServer> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("failed to bind retry integration test server")?;

    let address = listener
        .local_addr()
        .context("failed to read retry integration test server address")?;

    let requests = Arc::new(AtomicUsize::new(0));
    let connections = Arc::new(AtomicUsize::new(0));
    let statuses = Arc::new(statuses);

    let requests_for_task = Arc::clone(&requests);
    let connections_for_task = Arc::clone(&connections);
    let statuses_for_task = Arc::clone(&statuses);

    let handle = tokio::spawn(async move {
        loop {
            let Ok((socket, _peer)) = listener.accept().await else {
                break;
            };

            connections_for_task.fetch_add(1, Ordering::SeqCst);

            let requests = Arc::clone(&requests_for_task);
            let statuses = Arc::clone(&statuses_for_task);

            tokio::spawn(async move {
                handle_connection(socket, requests, statuses, keep_alive, response_delay).await;
            });
        }
    });

    Ok(TestServer {
        url: format!("http://{address}"),
        requests,
        connections,
        handle,
    })
}

async fn handle_connection(
    mut socket: TcpStream,
    requests: Arc<AtomicUsize>,
    statuses: Arc<Vec<u16>>,
    keep_alive: bool,
    response_delay: Duration,
) {
    loop {
        match read_http_request(&mut socket).await {
            Ok(true) => {}
            Ok(false) | Err(_) => break,
        }

        let request_index = requests.fetch_add(1, Ordering::SeqCst);
        let status = statuses
            .get(request_index)
            .copied()
            .or_else(|| statuses.last().copied())
            .unwrap_or(200);

        if !response_delay.is_zero() {
            tokio::time::sleep(response_delay).await;
        }

        let body = if status < 400 { "ok" } else { "error" };
        let connection = if keep_alive { "keep-alive" } else { "close" };
        let response = format!(
            "HTTP/1.1 {status} {}\r\nContent-Length: {}\r\nConnection: {connection}\r\n\r\n{body}",
            reason_phrase(status),
            body.len()
        );

        if socket.write_all(response.as_bytes()).await.is_err() {
            break;
        }

        if socket.flush().await.is_err() {
            break;
        }

        if !keep_alive {
            let _ = socket.shutdown().await;
            break;
        }
    }
}

async fn read_http_request(socket: &mut TcpStream) -> io::Result<bool> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];

    loop {
        let bytes_read = socket.read(&mut chunk).await?;

        if bytes_read == 0 {
            return Ok(false);
        }

        buffer.extend_from_slice(&chunk[..bytes_read]);

        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(true);
        }

        if buffer.len() > 16 * 1024 {
            return Ok(false);
        }
    }
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "OK",
    }
}

fn clank_binary() -> &'static str {
    env!("CARGO_BIN_EXE_clank-cli")
}

fn run_clank_json(args: &[&str]) -> Result<Value> {
    let output = Command::new(clank_binary())
        .args(args)
        .output()
        .context("failed to run clank-cli binary")?;

    let stdout = String::from_utf8(output.stdout).context("clank-cli stdout is not UTF-8")?;
    let stderr = String::from_utf8(output.stderr).context("clank-cli stderr is not UTF-8")?;

    if !output.status.success() {
        bail!(
            "clank-cli exited with status {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            stdout,
            stderr
        );
    }

    serde_json::from_str(&stdout).with_context(|| {
        format!("failed to parse clank-cli JSON output\nstdout:\n{stdout}\nstderr:\n{stderr}")
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_retries_transient_503_until_success() -> Result<()> {
    let server = start_status_server(vec![503, 200], true, Duration::ZERO).await?;

    let output = run_clank_json(&[
        server.url(),
        "--requests",
        "1",
        "--concurrency",
        "1",
        "--retry",
        "3",
        "--quiet",
        "--output",
        "json",
        "--no-color",
    ])?;

    assert_eq!(server.request_count(), 2);
    assert_eq!(output["total_requests"], 1);
    assert_eq!(output["successful"], 1);
    assert_eq!(output["errors"], 0);
    assert_eq!(output["retries"], 1);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_reports_errors_when_retry_is_exhausted() -> Result<()> {
    let server = start_status_server(vec![503], true, Duration::ZERO).await?;

    let output = run_clank_json(&[
        server.url(),
        "--requests",
        "3",
        "--concurrency",
        "1",
        "--retry",
        "2",
        "--quiet",
        "--output",
        "json",
        "--no-color",
    ])?;

    assert_eq!(server.request_count(), 9);
    assert_eq!(output["total_requests"], 3);
    assert_eq!(output["successful"], 0);
    assert_eq!(output["errors"], 3);
    assert_eq!(output["retries"], 6);
    assert_eq!(output["error_breakdown"]["http_5xx"], 3);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_does_not_retry_client_4xx() -> Result<()> {
    let server = start_status_server(vec![404, 200], true, Duration::ZERO).await?;

    let output = run_clank_json(&[
        server.url(),
        "--requests",
        "1",
        "--concurrency",
        "1",
        "--retry",
        "3",
        "--quiet",
        "--output",
        "json",
        "--no-color",
    ])?;

    assert_eq!(server.request_count(), 1);
    assert_eq!(output["total_requests"], 1);
    assert_eq!(output["successful"], 0);
    assert_eq!(output["errors"], 1);
    assert_eq!(output["retries"], 0);
    assert_eq!(output["error_breakdown"]["http_4xx"], 1);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_respects_retry_delay_between_attempts() -> Result<()> {
    let server = start_status_server(vec![503, 503, 200], true, Duration::ZERO).await?;
    let started_at = Instant::now();

    let output = run_clank_json(&[
        server.url(),
        "--requests",
        "1",
        "--concurrency",
        "1",
        "--retry",
        "2",
        "--retry-delay",
        "100ms",
        "--quiet",
        "--output",
        "json",
        "--no-color",
    ])?;

    let elapsed = started_at.elapsed();

    assert_eq!(server.request_count(), 3);
    assert_eq!(output["total_requests"], 1);
    assert_eq!(output["successful"], 1);
    assert_eq!(output["errors"], 0);
    assert_eq!(output["retries"], 2);
    assert!(
        elapsed >= Duration::from_millis(150),
        "expected at least about 200ms from two retry delays, got {elapsed:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_keep_alive_reuses_connections_and_no_keep_alive_opens_more_connections() -> Result<()>
{
    let keep_alive_server = start_status_server(vec![200], true, Duration::ZERO).await?;

    let keep_alive_output = run_clank_json(&[
        keep_alive_server.url(),
        "--requests",
        "4",
        "--concurrency",
        "1",
        "--keep-alive",
        "--quiet",
        "--output",
        "json",
        "--no-color",
    ])?;

    let no_keep_alive_server = start_status_server(vec![200], true, Duration::ZERO).await?;

    let no_keep_alive_output = run_clank_json(&[
        no_keep_alive_server.url(),
        "--requests",
        "4",
        "--concurrency",
        "1",
        "--no-keep-alive",
        "--quiet",
        "--output",
        "json",
        "--no-color",
    ])?;

    assert_eq!(keep_alive_output["total_requests"], 4);
    assert_eq!(no_keep_alive_output["total_requests"], 4);
    assert_eq!(keep_alive_server.request_count(), 4);
    assert_eq!(no_keep_alive_server.request_count(), 4);

    assert!(
        keep_alive_server.connection_count() < no_keep_alive_server.connection_count(),
        "expected keep-alive to use fewer TCP connections; keep_alive={}, no_keep_alive={}",
        keep_alive_server.connection_count(),
        no_keep_alive_server.connection_count()
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_retry_and_rate_limit_combo_keeps_logical_requests_rate_limited() -> Result<()> {
    let server = start_status_server(vec![503], true, Duration::ZERO).await?;
    let started_at = Instant::now();

    let output = run_clank_json(&[
        server.url(),
        "--requests",
        "3",
        "--concurrency",
        "1",
        "--retry",
        "2",
        "--rate-limit",
        "2/s",
        "--quiet",
        "--output",
        "json",
        "--no-color",
    ])?;

    let elapsed = started_at.elapsed();

    assert_eq!(output["total_requests"], 3);
    assert_eq!(output["errors"], 3);
    assert_eq!(output["retries"], 6);
    assert_eq!(output["rate_limit"], "2/s");
    assert_eq!(server.request_count(), 9);

    assert!(
        elapsed >= Duration::from_millis(400),
        "expected third logical request to wait for the 2/s rate limit, got {elapsed:?}"
    );

    Ok(())
}
