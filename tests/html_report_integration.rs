use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

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

struct ClankOutput {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

async fn start_html_report_server() -> Result<TestServer> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("failed to bind HTML report test server")?;

    let address = listener
        .local_addr()
        .context("failed to read HTML report test server address")?;

    let requests = Arc::new(AtomicUsize::new(0));
    let requests_for_task = Arc::clone(&requests);

    let handle = tokio::spawn(async move {
        loop {
            let Ok((socket, _peer)) = listener.accept().await else {
                break;
            };

            let requests = Arc::clone(&requests_for_task);

            tokio::spawn(async move {
                handle_connection(socket, requests).await;
            });
        }
    });

    Ok(TestServer {
        url: format!("http://{address}"),
        requests,
        handle,
    })
}

async fn handle_connection(mut socket: TcpStream, requests: Arc<AtomicUsize>) {
    loop {
        match read_http_request(&mut socket).await {
            Ok(true) => {}
            Ok(false) | Err(_) => break,
        }

        requests.fetch_add(1, Ordering::SeqCst);

        let body = "ok";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: keep-alive\r\nContent-Type: text/plain\r\n\r\n{body}",
            body.len()
        );

        if socket.write_all(response.as_bytes()).await.is_err() {
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

fn clank_binary() -> &'static str {
    env!("CARGO_BIN_EXE_clank-cli")
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

fn run_clank_in_dir(args: &[&str], current_dir: &Path) -> Result<ClankOutput> {
    let output = Command::new(clank_binary())
        .current_dir(current_dir)
        .args(args)
        .output()
        .context("failed to run clank-cli binary")?;

    Ok(ClankOutput {
        status: output.status,
        stdout: String::from_utf8(output.stdout).context("clank-cli stdout is not UTF-8")?,
        stderr: String::from_utf8(output.stderr).context("clank-cli stderr is not UTF-8")?,
    })
}

fn unique_temp_path(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    std::env::temp_dir().join(format!("clank-cli-{name}-{nanos}.html"))
}

fn unique_temp_dir(name: &str) -> Result<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    let path = std::env::temp_dir().join(format!("clank-cli-{name}-{nanos}"));
    fs::create_dir_all(&path).with_context(|| format!("failed to create {}", path.display()))?;

    Ok(path)
}

fn parse_json_stdout(output: &ClankOutput) -> Result<Value> {
    serde_json::from_str(&output.stdout).with_context(|| {
        format!(
            "failed to parse JSON stdout\nstdout:\n{}\nstderr:\n{}",
            output.stdout, output.stderr
        )
    })
}

fn extract_report_data(html: &str) -> Result<Value> {
    let marker = r#"<script id="report-data" type="application/json">"#;
    let start = html
        .find(marker)
        .map(|index| index + marker.len())
        .context("missing report-data script tag")?;

    let end = html[start..]
        .find("</script>")
        .map(|index| start + index)
        .context("missing report-data closing script tag")?;

    let json = html[start..end].trim();

    serde_json::from_str(json)
        .with_context(|| format!("failed to parse embedded report JSON\njson:\n{json}"))
}

fn assert_success(output: &ClankOutput) -> Result<()> {
    if !output.status.success() {
        bail!(
            "clank-cli failed\nstdout:\n{}\nstderr:\n{}",
            output.stdout,
            output.stderr
        );
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_writes_html_report_to_explicit_output_file() -> Result<()> {
    let server = start_html_report_server().await?;
    let report_path = unique_temp_path("explicit-report");

    let output = run_clank(&[
        server.url(),
        "--requests",
        "1",
        "--concurrency",
        "1",
        "--quiet",
        "--output",
        "html",
        "--output-file",
        report_path
            .to_str()
            .context("report path is not valid UTF-8")?,
        "--no-color",
    ])?;

    assert_success(&output)?;

    assert_eq!(server.request_count(), 1);
    assert!(output.stdout.contains("HTML report written to"));
    assert!(output.stdout.contains(report_path.to_str().unwrap()));

    let html = fs::read_to_string(&report_path)
        .with_context(|| format!("failed to read {}", report_path.display()))?;

    let metadata = fs::metadata(&report_path)
        .with_context(|| format!("failed to stat {}", report_path.display()))?;

    assert!(
        metadata.len() > 50 * 1024,
        "HTML report should include template and bundled Chart.js, got {} bytes",
        metadata.len()
    );
    assert!(html.contains("<!doctype html>"));
    assert!(html.contains(r#"id="report-data""#));
    assert!(html.contains("Chart.js v4.5.1"));
    assert!(html.contains(server.url()));

    fs::remove_file(report_path).ok();

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_html_report_contains_all_sections() -> Result<()> {
    let server = start_html_report_server().await?;
    let report_path = unique_temp_path("sections-report");

    let output = run_clank(&[
        server.url(),
        "--requests",
        "1",
        "--concurrency",
        "1",
        "--quiet",
        "--output",
        "html",
        "--output-file",
        report_path
            .to_str()
            .context("report path is not valid UTF-8")?,
        "--no-color",
    ])?;

    assert_success(&output)?;

    let html = fs::read_to_string(&report_path)
        .with_context(|| format!("failed to read {}", report_path.display()))?;

    assert!(html.contains("Load Test Report"));
    assert!(html.contains("Total Requests"));
    assert!(html.contains("Throughput"));
    assert!(html.contains("Success Rate"));
    assert!(html.contains("Total Errors"));
    assert!(html.contains("Latency Percentiles"));
    assert!(html.contains("Latency Histogram"));
    assert!(html.contains("Status Codes"));
    assert!(html.contains("Error Distribution"));
    assert!(html.contains("Response Validation"));
    assert!(html.contains("<canvas id=\"status-chart\""));
    assert!(html.contains("<canvas id=\"latency-chart\""));
    assert!(html.contains("<canvas id=\"error-chart\""));
    assert!(html.contains(r#"<script id="report-data" type="application/json">"#));

    let report_data = extract_report_data(&html)?;
    assert!(report_data["metadata"].is_object());
    assert!(report_data["summary"].is_object());
    assert!(report_data["latency_histogram"].is_array());
    assert!(report_data["status_codes"].is_array());

    fs::remove_file(report_path).ok();

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_writes_html_report_to_default_output_file() -> Result<()> {
    let server = start_html_report_server().await?;
    let temp_dir = unique_temp_dir("default-report")?;

    let output = run_clank_in_dir(
        &[
            server.url(),
            "--requests",
            "1",
            "--concurrency",
            "1",
            "--quiet",
            "--output",
            "html",
            "--no-color",
        ],
        &temp_dir,
    )?;

    assert_success(&output)?;

    assert!(output.stdout.contains("HTML report written to"));

    let report_files = fs::read_dir(&temp_dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "html")
        })
        .collect::<Vec<_>>();

    assert_eq!(report_files.len(), 1);

    let html = fs::read_to_string(&report_files[0])?;
    assert!(html.contains("<!doctype html>"));
    assert!(html.contains(r#""metadata""#));
    assert!(html.contains(r#""summary""#));

    fs::remove_dir_all(temp_dir).ok();

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_keeps_json_output_working() -> Result<()> {
    let server = start_html_report_server().await?;

    let output = run_clank(&[
        server.url(),
        "--requests",
        "1",
        "--concurrency",
        "1",
        "--quiet",
        "--output",
        "json",
        "--no-color",
    ])?;

    assert_success(&output)?;

    let json = parse_json_stdout(&output)?;

    assert_eq!(json["total_requests"], 1);
    assert_eq!(json["successful"], 1);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_html_report_error_on_write_failure() -> Result<()> {
    let server = start_html_report_server().await?;
    let report_path = unique_temp_dir("write-failure-report")?;

    let output = run_clank(&[
        server.url(),
        "--requests",
        "1",
        "--concurrency",
        "1",
        "--quiet",
        "--output",
        "html",
        "--output-file",
        report_path
            .to_str()
            .context("report path is not valid UTF-8")?,
        "--no-color",
    ])?;

    assert!(!output.status.success());
    assert!(
        output.stderr.contains("failed to write HTML report"),
        "stderr should mention write failure, got:\n{}",
        output.stderr
    );

    fs::remove_dir_all(report_path).ok();

    Ok(())
}

#[test]
fn cli_rejects_duplicate_output_flags() -> Result<()> {
    let output = run_clank(&[
        "--output",
        "json",
        "--output",
        "html",
        "http://127.0.0.1:1",
        "--requests",
        "1",
        "--quiet",
        "--no-color",
    ])?;

    assert!(!output.status.success());
    assert!(output.stderr.contains("--output"));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_html_report_data_accuracy() -> Result<()> {
    let server = start_html_report_server().await?;
    let report_path = unique_temp_path("data-accuracy-report");

    let output = run_clank(&[
        server.url(),
        "--requests",
        "3",
        "--concurrency",
        "1",
        "--quiet",
        "--output",
        "html",
        "--output-file",
        report_path
            .to_str()
            .context("report path is not valid UTF-8")?,
        "--no-color",
    ])?;

    assert_success(&output)?;
    assert_eq!(server.request_count(), 3);

    let html = fs::read_to_string(&report_path)
        .with_context(|| format!("failed to read {}", report_path.display()))?;
    let report_data = extract_report_data(&html)?;

    assert_eq!(report_data["metadata"]["target_url"], server.url());
    assert_eq!(report_data["metadata"]["concurrency"], 1);
    assert_eq!(report_data["summary"]["total_requests"], 3);
    assert_eq!(report_data["summary"]["successful"], 3);
    assert_eq!(report_data["summary"]["failed"], 0);
    assert_eq!(report_data["summary"]["validation_errors"], 0);

    let rps = report_data["summary"]["rps"]
        .as_f64()
        .context("summary.rps should be a number")?;
    assert!(rps >= 0.0);

    let p50 = report_data["latency"]["p50_ms"]
        .as_f64()
        .context("latency.p50_ms should be a number")?;
    let p99 = report_data["latency"]["p99_ms"]
        .as_f64()
        .context("latency.p99_ms should be a number")?;
    assert!(p99 >= p50);

    let status_codes = report_data["status_codes"]
        .as_array()
        .context("status_codes should be an array")?;
    let status_200 = status_codes
        .iter()
        .find(|status| status["code"] == 200)
        .context("status 200 should exist")?;
    assert_eq!(status_200["count"], 3);

    let latency_histogram_total = report_data["latency_histogram"]
        .as_array()
        .context("latency_histogram should be an array")?
        .iter()
        .map(|bucket| bucket["count"].as_u64().unwrap_or(0))
        .sum::<u64>();

    assert_eq!(latency_histogram_total, 3);

    fs::remove_file(report_path).ok();

    Ok(())
}
