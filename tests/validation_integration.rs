use std::io;
use std::process::{Command, ExitStatus};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

#[derive(Debug, Clone)]
struct ResponseSpec {
    status: u16,
    body: String,
    headers: Vec<(String, String)>,
}

impl ResponseSpec {
    fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            headers: Vec::new(),
        }
    }

    fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

struct TestServer {
    url: String,
    requests: Arc<AtomicUsize>,
    handle: JoinHandle<()>,
}

impl TestServer {
    fn url(&self) -> &str {
        &self.url
    }

    fn request_count(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn start_validation_server(responses: Vec<ResponseSpec>) -> Result<TestServer> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("failed to bind validation integration test server")?;

    let address = listener
        .local_addr()
        .context("failed to read validation integration test server address")?;

    let requests = Arc::new(AtomicUsize::new(0));
    let responses = Arc::new(responses);

    let requests_for_task = Arc::clone(&requests);
    let responses_for_task = Arc::clone(&responses);

    let handle = tokio::spawn(async move {
        loop {
            let Ok((socket, _peer)) = listener.accept().await else {
                break;
            };

            let requests = Arc::clone(&requests_for_task);
            let responses = Arc::clone(&responses_for_task);

            tokio::spawn(async move {
                handle_connection(socket, requests, responses).await;
            });
        }
    });

    Ok(TestServer {
        url: format!("http://{address}"),
        requests,
        handle,
    })
}

async fn handle_connection(
    mut socket: TcpStream,
    requests: Arc<AtomicUsize>,
    responses: Arc<Vec<ResponseSpec>>,
) {
    loop {
        match read_http_request(&mut socket).await {
            Ok(true) => {}
            Ok(false) | Err(_) => break,
        }

        let request_index = requests.fetch_add(1, Ordering::SeqCst);
        let response = responses
            .get(request_index)
            .or_else(|| responses.last())
            .cloned()
            .unwrap_or_else(|| ResponseSpec::new(200, "ok"));

        let mut headers = String::new();

        for (name, value) in &response.headers {
            headers.push_str(name);
            headers.push_str(": ");
            headers.push_str(value);
            headers.push_str("\r\n");
        }

        let body_len = response.body.len();
        let raw_response = format!(
            "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: keep-alive\r\n{}\
             \r\n{}",
            response.status,
            reason_phrase(response.status),
            body_len,
            headers,
            response.body
        );

        if socket.write_all(raw_response.as_bytes()).await.is_err() {
            break;
        }

        if socket.flush().await.is_err() {
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
        201 => "Created",
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

struct ClankOutput {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

fn run_clank(args: &[&str]) -> Result<ClankOutput> {
    let output = Command::new(clank_binary())
        .args(args)
        .output()
        .context("failed to run clank-cli binary")?;

    Ok(ClankOutput {
        status: output.status,
        stdout: String::from_utf8(output.stdout).context("clank-cli stdout is not UTF-8")?,
        stderr: String::from_utf8(output.stderr).context("clank-cli stderr is not UTF-8")?,
    })
}

fn parse_json_output(output: &ClankOutput) -> Result<Value> {
    serde_json::from_str(&output.stdout).with_context(|| {
        format!(
            "failed to parse clank-cli JSON output\nstdout:\n{}\nstderr:\n{}",
            output.stdout, output.stderr
        )
    })
}

fn run_clank_json_expect_validation_failure(args: &[&str]) -> Result<(Value, String)> {
    let output = run_clank(args)?;

    if output.status.success() {
        bail!(
            "expected clank-cli to exit with validation failure\nstdout:\n{}\nstderr:\n{}",
            output.stdout,
            output.stderr
        );
    }

    let json = parse_json_output(&output)?;

    Ok((json, output.stderr))
}

fn run_clank_json(args: &[&str]) -> Result<Value> {
    let output = run_clank(args)?;

    if !output.status.success() {
        bail!(
            "clank-cli exited with status {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            output.stdout,
            output.stderr
        );
    }

    parse_json_output(&output)
}

fn validation_args<'a>(server_url: &'a str, extra_args: &'a [&'a str]) -> Vec<&'a str> {
    let mut args = vec![
        server_url,
        "--requests",
        "1",
        "--concurrency",
        "1",
        "--quiet",
        "--output",
        "json",
        "--no-color",
    ];

    args.extend_from_slice(extra_args);
    args
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_counts_wrong_status_code_as_validation_error() -> Result<()> {
    let server = start_validation_server(vec![ResponseSpec::new(404, "not found")]).await?;

    let (output, stderr) = run_clank_json_expect_validation_failure(&validation_args(
        server.url(),
        &["--expect-status", "200"],
    ))?;

    assert!(stderr.contains("Validation failed:"));
    assert!(stderr.contains("expected status"));

    assert_eq!(server.request_count(), 1);
    assert_eq!(output["total_requests"], 1);
    assert_eq!(output["successful"], 0);
    assert_eq!(output["errors"], 1);
    assert_eq!(output["validation_errors"], 1);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_accepts_correct_status_code() -> Result<()> {
    let server = start_validation_server(vec![ResponseSpec::new(200, "ok")]).await?;

    let output = run_clank_json(&validation_args(server.url(), &["--expect-status", "200"]))?;

    assert_eq!(server.request_count(), 1);
    assert_eq!(output["total_requests"], 1);
    assert_eq!(output["successful"], 1);
    assert_eq!(output["errors"], 0);
    assert_eq!(output["validation_errors"], 0);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_accepts_multiple_status_codes() -> Result<()> {
    let server = start_validation_server(vec![ResponseSpec::new(201, "created")]).await?;

    let output = run_clank_json(&validation_args(
        server.url(),
        &["--expect-status", "200,201"],
    ))?;

    assert_eq!(server.request_count(), 1);
    assert_eq!(output["total_requests"], 1);
    assert_eq!(output["successful"], 1);
    assert_eq!(output["errors"], 0);
    assert_eq!(output["validation_errors"], 0);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_accepts_correct_body_pattern() -> Result<()> {
    let server =
        start_validation_server(vec![ResponseSpec::new(200, r#"{"status":"ok"}"#)]).await?;

    let output = run_clank_json(&validation_args(
        server.url(),
        &["--expect-body", r#""status":"ok""#],
    ))?;

    assert_eq!(server.request_count(), 1);
    assert_eq!(output["successful"], 1);
    assert_eq!(output["errors"], 0);
    assert_eq!(output["validation_errors"], 0);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_counts_wrong_body_pattern_as_validation_error() -> Result<()> {
    let server =
        start_validation_server(vec![ResponseSpec::new(200, r#"{"status":"error"}"#)]).await?;

    let (output, stderr) = run_clank_json_expect_validation_failure(&validation_args(
        server.url(),
        &["--expect-body", r#""status":"ok""#],
    ))?;

    assert!(stderr.contains("Validation failed:"));
    assert!(stderr.contains("expected body to match pattern"));

    assert_eq!(server.request_count(), 1);
    assert_eq!(output["successful"], 0);
    assert_eq!(output["errors"], 1);
    assert_eq!(output["validation_errors"], 1);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_counts_invalid_regex_as_validation_error() -> Result<()> {
    let server = start_validation_server(vec![ResponseSpec::new(200, "hello")]).await?;

    let (output, stderr) = run_clank_json_expect_validation_failure(&validation_args(
        server.url(),
        &["--expect-body", "[invalid"],
    ))?;

    assert_eq!(server.request_count(), 1);
    assert_eq!(output["total_requests"], 1);
    assert_eq!(output["successful"], 0);
    assert_eq!(output["errors"], 1);
    assert_eq!(output["validation_errors"], 1);

    assert!(stderr.contains("Validation failed:"));
    assert!(stderr.contains("invalid body regex"));
    assert!(stderr.contains("[invalid"));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_handles_complex_valid_regex() -> Result<()> {
    let server =
        start_validation_server(vec![ResponseSpec::new(200, "status: ok, count: 42")]).await?;

    let digit_output = run_clank_json(&validation_args(server.url(), &["--expect-body", "\\d+"]))?;

    assert_eq!(digit_output["successful"], 1);
    assert_eq!(digit_output["errors"], 0);
    assert_eq!(digit_output["validation_errors"], 0);

    let prefix_output = run_clank_json(&validation_args(
        server.url(),
        &["--expect-body", "^status"],
    ))?;

    assert_eq!(prefix_output["successful"], 1);
    assert_eq!(prefix_output["errors"], 0);
    assert_eq!(prefix_output["validation_errors"], 0);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_accepts_correct_header_with_case_insensitive_key() -> Result<()> {
    let server = start_validation_server(vec![
        ResponseSpec::new(200, "ok").with_header("content-type", "application/json"),
    ])
    .await?;

    let output = run_clank_json(&validation_args(
        server.url(),
        &["--expect-header", "Content-Type: application/json"],
    ))?;

    assert_eq!(server.request_count(), 1);
    assert_eq!(output["successful"], 1);
    assert_eq!(output["errors"], 0);
    assert_eq!(output["validation_errors"], 0);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_counts_header_value_mismatch_as_validation_error() -> Result<()> {
    let server = start_validation_server(vec![
        ResponseSpec::new(200, "ok").with_header("Content-Type", "text/html"),
    ])
    .await?;

    let (output, stderr) = run_clank_json_expect_validation_failure(&validation_args(
        server.url(),
        &["--expect-header", "Content-Type: application/json"],
    ))?;

    assert!(stderr.contains("Validation failed:"));
    assert!(stderr.contains("expected header"));
    assert!(stderr.contains("Content-Type"));
    assert!(stderr.contains("application/json"));
    assert!(stderr.contains("text/html"));

    assert_eq!(server.request_count(), 1);
    assert_eq!(output["total_requests"], 1);
    assert_eq!(output["successful"], 0);
    assert_eq!(output["errors"], 1);
    assert_eq!(output["validation_errors"], 1);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_counts_missing_header_as_validation_error() -> Result<()> {
    let server = start_validation_server(vec![ResponseSpec::new(200, "ok")]).await?;

    let (output, stderr) = run_clank_json_expect_validation_failure(&validation_args(
        server.url(),
        &["--expect-header", "X-Custom: some-value"],
    ))?;

    assert!(stderr.contains("Validation failed:"));
    assert!(stderr.contains("expected header"));
    assert!(stderr.contains("X-Custom"));
    assert!(stderr.contains("some-value"));

    assert_eq!(server.request_count(), 1);
    assert_eq!(output["total_requests"], 1);
    assert_eq!(output["successful"], 0);
    assert_eq!(output["errors"], 1);
    assert_eq!(output["validation_errors"], 1);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_accepts_multiple_headers_all_match() -> Result<()> {
    let server = start_validation_server(vec![
        ResponseSpec::new(200, "ok")
            .with_header("Content-Type", "application/json")
            .with_header("X-Custom", "some-value"),
    ])
    .await?;

    let output = run_clank_json(&validation_args(
        server.url(),
        &[
            "--expect-header",
            "Content-Type: application/json",
            "--expect-header",
            "X-Custom: some-value",
        ],
    ))?;

    assert_eq!(server.request_count(), 1);
    assert_eq!(output["total_requests"], 1);
    assert_eq!(output["successful"], 1);
    assert_eq!(output["errors"], 0);
    assert_eq!(output["validation_errors"], 0);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_accepts_multiple_validation_rules_together() -> Result<()> {
    let server = start_validation_server(vec![
        ResponseSpec::new(200, r#"{"status":"ok"}"#)
            .with_header("Content-Type", "application/json"),
    ])
    .await?;

    let output = run_clank_json(&validation_args(
        server.url(),
        &[
            "--expect-status",
            "200",
            "--expect-body",
            "ok",
            "--expect-header",
            "Content-Type: application/json",
        ],
    ))?;

    assert_eq!(server.request_count(), 1);
    assert_eq!(output["successful"], 1);
    assert_eq!(output["errors"], 0);
    assert_eq!(output["validation_errors"], 0);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_applies_validation_after_retry_final_response() -> Result<()> {
    let server = start_validation_server(vec![
        ResponseSpec::new(503, "temporary"),
        ResponseSpec::new(200, "wrong"),
    ])
    .await?;

    let (output, stderr) = run_clank_json_expect_validation_failure(&validation_args(
        server.url(),
        &["--retry", "2", "--expect-body", "ok"],
    ))?;

    assert!(stderr.contains("Validation failed:"));
    assert!(stderr.contains("expected body to match pattern"));

    assert_eq!(server.request_count(), 2);
    assert_eq!(output["total_requests"], 1);
    assert_eq!(output["successful"], 0);
    assert_eq!(output["errors"], 1);
    assert_eq!(output["validation_errors"], 1);
    assert_eq!(output["retries"], 1);

    Ok(())
}
