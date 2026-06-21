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

    if !output.status.success() {
        bail!(
            "clank-cli failed\nstdout:\n{}\nstderr:\n{}",
            output.stdout,
            output.stderr
        );
    }

    assert_eq!(server.request_count(), 1);
    assert!(output.stdout.contains("HTML report written to"));
    assert!(output.stdout.contains(report_path.to_str().unwrap()));

    let html = fs::read_to_string(&report_path)
        .with_context(|| format!("failed to read {}", report_path.display()))?;

    assert!(html.contains("<!doctype html>"));
    assert!(html.contains(r#"id="report-data""#));
    assert!(html.contains("Chart.js v4.5.1"));
    assert!(html.contains(server.url()));

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

    if !output.status.success() {
        bail!(
            "clank-cli failed\nstdout:\n{}\nstderr:\n{}",
            output.stdout,
            output.stderr
        );
    }

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

    if !output.status.success() {
        bail!(
            "clank-cli failed\nstdout:\n{}\nstderr:\n{}",
            output.stdout,
            output.stderr
        );
    }

    let json = parse_json_stdout(&output)?;

    assert_eq!(json["total_requests"], 1);
    assert_eq!(json["successful"], 1);

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
