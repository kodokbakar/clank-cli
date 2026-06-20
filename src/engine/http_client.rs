use std::error::Error;
use std::fmt;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use reqwest::{
    Client, Method, StatusCode,
    header::{HeaderMap, HeaderName, HeaderValue},
};

#[derive(Debug, Clone)]
pub struct HttpClient {
    client: Client,
    keep_alive: bool,
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: String,
    pub latency: Duration,
    pub retry_stats: RetryStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryConfig {
    pub max_retries: usize,
    pub delay: Duration,
}

impl RetryConfig {
    pub fn disabled() -> Self {
        Self {
            max_retries: 0,
            delay: Duration::ZERO,
        }
    }
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self::disabled()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RetryStats {
    pub total_attempts: usize,
    pub retried: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpErrorKind {
    Timeout,
    Connection,
    Request,
    UnsupportedMethod,
    Other,
}

#[derive(Debug, Clone)]
pub struct HttpClientError {
    kind: HttpErrorKind,
    message: String,
    retry_stats: RetryStats,
}

impl HttpClientError {
    pub fn new(kind: HttpErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            retry_stats: RetryStats::default(),
        }
    }

    pub fn kind(&self) -> HttpErrorKind {
        self.kind
    }

    pub fn retry_stats(&self) -> RetryStats {
        self.retry_stats
    }

    fn with_retry_stats(mut self, retry_stats: RetryStats) -> Self {
        self.retry_stats = retry_stats;
        self
    }
}

impl fmt::Display for HttpClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl Error for HttpClientError {}

impl HttpClient {
    pub fn new(timeout: Duration, insecure: bool, keep_alive: bool) -> Result<Self> {
        let mut builder = Client::builder()
            .timeout(timeout)
            .danger_accept_invalid_certs(insecure);

        if !keep_alive {
            builder = builder.pool_max_idle_per_host(0);
        }

        let client = builder.build().context("failed to build HTTP client")?;

        Ok(Self { client, keep_alive })
    }

    pub fn keep_alive(&self) -> bool {
        self.keep_alive
    }

    pub async fn send(
        &self,
        method: &str,
        url: &str,
        body: Option<String>,
        headers: &[(String, String)],
        retry_config: RetryConfig,
    ) -> std::result::Result<HttpResponse, HttpClientError> {
        let normalized_method = method.to_ascii_uppercase();
        let max_attempts = retry_config.max_retries.saturating_add(1);
        let mut total_attempts = 0;

        loop {
            total_attempts += 1;
            let attempt_started_at = Instant::now();

            let request = self.build_request(&normalized_method, url, body.clone(), headers)?;

            let response = match request.send().await {
                Ok(response) => response,
                Err(error) => {
                    let error = classify_reqwest_error(error);

                    if should_retry_error(error.kind()) && total_attempts < max_attempts {
                        sleep_before_retry(retry_config.delay).await;
                        continue;
                    }

                    return Err(error.with_retry_stats(retry_stats(total_attempts)));
                }
            };

            let status = response.status();
            let response_headers = response.headers().clone();

            let response_body = match response.text().await {
                Ok(body) => body,
                Err(error) => {
                    let error = classify_reqwest_error(error);

                    if should_retry_error(error.kind()) && total_attempts < max_attempts {
                        sleep_before_retry(retry_config.delay).await;
                        continue;
                    }

                    return Err(error.with_retry_stats(retry_stats(total_attempts)));
                }
            };

            if status.is_server_error() && total_attempts < max_attempts {
                sleep_before_retry(retry_config.delay).await;
                continue;
            }

            return Ok(HttpResponse {
                status,
                headers: response_headers,
                body: response_body,
                latency: attempt_started_at.elapsed(),
                retry_stats: retry_stats(total_attempts),
            });
        }
    }

    fn build_request(
        &self,
        normalized_method: &str,
        url: &str,
        body: Option<String>,
        headers: &[(String, String)],
    ) -> std::result::Result<reqwest::RequestBuilder, HttpClientError> {
        let request = match normalized_method {
            "GET" => self.client.get(url),
            "POST" => self.client.post(url),
            "PUT" => self.client.put(url),
            "DELETE" => self.client.delete(url),
            "PATCH" => self.client.patch(url),
            "HEAD" => self.client.head(url),
            "OPTIONS" => self.client.request(Method::OPTIONS, url),
            _ => {
                return Err(HttpClientError::new(
                    HttpErrorKind::UnsupportedMethod,
                    format!("unsupported method: {normalized_method}"),
                ));
            }
        };

        let request = if normalized_method == "HEAD" {
            request
        } else {
            match body {
                Some(body) => request.body(body),
                None => request,
            }
        };

        apply_headers(request, headers)
    }
}

fn retry_stats(total_attempts: usize) -> RetryStats {
    RetryStats {
        total_attempts,
        retried: total_attempts > 1,
    }
}

fn should_retry_error(kind: HttpErrorKind) -> bool {
    matches!(kind, HttpErrorKind::Connection | HttpErrorKind::Timeout)
}

async fn sleep_before_retry(delay: Duration) {
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
}

fn apply_headers(
    mut request: reqwest::RequestBuilder,
    headers: &[(String, String)],
) -> std::result::Result<reqwest::RequestBuilder, HttpClientError> {
    for (key, value) in headers {
        let name = HeaderName::from_bytes(key.as_bytes()).map_err(|error| {
            HttpClientError::new(
                HttpErrorKind::Request,
                format!("invalid header name `{key}`: {error}"),
            )
        })?;

        let value = HeaderValue::from_str(value).map_err(|error| {
            HttpClientError::new(
                HttpErrorKind::Request,
                format!("invalid header value for `{key}`: {error}"),
            )
        })?;

        request = request.header(name, value);
    }

    Ok(request)
}

fn classify_reqwest_error(error: reqwest::Error) -> HttpClientError {
    if error.is_timeout() {
        HttpClientError::new(
            HttpErrorKind::Timeout,
            format!("request timed out: {error}"),
        )
    } else if error.is_connect() {
        HttpClientError::new(
            HttpErrorKind::Connection,
            format!("connection failed, DNS failed, or target refused connection: {error}"),
        )
    } else if error.is_request() {
        HttpClientError::new(HttpErrorKind::Request, format!("invalid request: {error}"))
    } else {
        HttpClientError::new(HttpErrorKind::Other, format!("HTTP client error: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use anyhow::Result;
    use httpmock::prelude::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn start_retry_status_server(statuses: Vec<u16>) -> Result<(String, Arc<AtomicUsize>)> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let attempts = Arc::new(AtomicUsize::new(0));
        let statuses = Arc::new(statuses);

        let attempts_for_task = Arc::clone(&attempts);
        let statuses_for_task = Arc::clone(&statuses);

        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _peer)) = listener.accept().await else {
                    break;
                };

                let attempts = Arc::clone(&attempts_for_task);
                let statuses = Arc::clone(&statuses_for_task);

                tokio::spawn(async move {
                    let mut buffer = [0_u8; 1024];
                    let _ = socket.read(&mut buffer).await;

                    let attempt_index = attempts.fetch_add(1, Ordering::SeqCst);
                    let status = statuses
                        .get(attempt_index)
                        .copied()
                        .or_else(|| statuses.last().copied())
                        .unwrap_or(200);

                    let reason = match status {
                        200 => "OK",
                        400 => "Bad Request",
                        404 => "Not Found",
                        500 => "Internal Server Error",
                        502 => "Bad Gateway",
                        503 => "Service Unavailable",
                        _ => "OK",
                    };

                    let body = if status == 200 { "ok" } else { "error" };
                    let response = format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );

                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });

        Ok((format!("http://{address}/flaky"), attempts))
    }

    #[tokio::test]
    async fn send_retries_503_until_final_200() -> Result<()> {
        let (url, attempts) = start_retry_status_server(vec![503, 200]).await?;
        let client = HttpClient::new(Duration::from_secs(10), false, true)?;

        let response = client
            .send(
                "GET",
                &url,
                None,
                &[],
                RetryConfig {
                    max_retries: 3,
                    delay: Duration::ZERO,
                },
            )
            .await?;

        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, "ok");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(
            response.retry_stats,
            RetryStats {
                total_attempts: 2,
                retried: true,
            }
        );

        Ok(())
    }

    #[tokio::test]
    async fn send_returns_final_503_after_retry_exhausted() -> Result<()> {
        let (url, attempts) = start_retry_status_server(vec![503, 503, 503]).await?;
        let client = HttpClient::new(Duration::from_secs(10), false, true)?;

        let response = client
            .send(
                "GET",
                &url,
                None,
                &[],
                RetryConfig {
                    max_retries: 2,
                    delay: Duration::ZERO,
                },
            )
            .await?;

        assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        assert_eq!(
            response.retry_stats,
            RetryStats {
                total_attempts: 3,
                retried: true,
            }
        );

        Ok(())
    }

    #[tokio::test]
    async fn send_does_not_retry_4xx() -> Result<()> {
        let (url, attempts) = start_retry_status_server(vec![404, 200]).await?;
        let client = HttpClient::new(Duration::from_secs(10), false, true)?;

        let response = client
            .send(
                "GET",
                &url,
                None,
                &[],
                RetryConfig {
                    max_retries: 3,
                    delay: Duration::ZERO,
                },
            )
            .await?;

        assert_eq!(response.status, StatusCode::NOT_FOUND);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(
            response.retry_stats,
            RetryStats {
                total_attempts: 1,
                retried: false,
            }
        );

        Ok(())
    }

    #[tokio::test]
    async fn send_respects_retry_delay() -> Result<()> {
        let (url, attempts) = start_retry_status_server(vec![503, 200]).await?;
        let client = HttpClient::new(Duration::from_secs(10), false, true)?;
        let started_at = Instant::now();

        let response = client
            .send(
                "GET",
                &url,
                None,
                &[],
                RetryConfig {
                    max_retries: 1,
                    delay: Duration::from_millis(75),
                },
            )
            .await?;

        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert!(started_at.elapsed() >= Duration::from_millis(75));

        Ok(())
    }

    #[tokio::test]
    async fn send_retry_latency_excludes_retry_delay() -> Result<()> {
        let (url, attempts) = start_retry_status_server(vec![503, 200]).await?;
        let client = HttpClient::new(Duration::from_secs(10), false, true)?;

        let response = client
            .send(
                "GET",
                &url,
                None,
                &[],
                RetryConfig {
                    max_retries: 1,
                    delay: Duration::from_millis(150),
                },
            )
            .await?;

        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert!(
            response.latency < Duration::from_millis(150),
            "final-attempt latency should not include retry delay, got {:?}",
            response.latency
        );

        Ok(())
    }

    #[tokio::test]
    async fn send_retry_zero_keeps_backward_compatible_single_attempt() -> Result<()> {
        let (url, attempts) = start_retry_status_server(vec![503, 200]).await?;
        let client = HttpClient::new(Duration::from_secs(10), false, true)?;

        let response = client
            .send("GET", &url, None, &[], RetryConfig::disabled())
            .await?;

        assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(
            response.retry_stats,
            RetryStats {
                total_attempts: 1,
                retried: false,
            }
        );

        Ok(())
    }

    #[tokio::test]
    async fn send_get_returns_status_headers_body_and_latency() -> Result<()> {
        let server = MockServer::start_async().await;

        let mock = server
            .mock_async(|when, then| {
                when.method(GET).path("/get");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(r#"{"ok":true}"#);
            })
            .await;

        let client = HttpClient::new(Duration::from_secs(10), false, true)?;
        let result = client
            .send(
                "GET",
                &server.url("/get"),
                None,
                &[],
                RetryConfig::disabled(),
            )
            .await?;

        assert_eq!(result.status, StatusCode::OK);
        assert!(result.headers.contains_key("content-type"));
        assert_eq!(result.body, r#"{"ok":true}"#);
        assert!(result.latency > Duration::ZERO);
        assert_eq!(mock.calls_async().await, 1);

        Ok(())
    }

    #[tokio::test]
    async fn send_get_without_body_returns_response() -> Result<()> {
        let server = MockServer::start_async().await;

        let mock = server
            .mock_async(|when, then| {
                when.method("GET").path("/get-no-body");
                then.status(200).body("ok");
            })
            .await;

        let client = HttpClient::new(Duration::from_secs(10), false, true)?;
        let result = client
            .send(
                "GET",
                &server.url("/get-no-body"),
                None,
                &[],
                RetryConfig::disabled(),
            )
            .await?;

        assert_eq!(result.status, StatusCode::OK);
        assert_eq!(result.body, "ok");
        assert_eq!(mock.calls_async().await, 1);

        Ok(())
    }

    #[tokio::test]
    async fn send_post_sends_body_and_returns_response() -> Result<()> {
        let server = MockServer::start_async().await;

        let mock = server
            .mock_async(|when, then| {
                when.method(POST).path("/post").body("hello");
                then.status(201).body("created");
            })
            .await;

        let client = HttpClient::new(Duration::from_secs(10), false, true)?;
        let result = client
            .send(
                "POST",
                &server.url("/post"),
                Some("hello".to_string()),
                &[],
                RetryConfig::disabled(),
            )
            .await?;

        assert_eq!(result.status, StatusCode::CREATED);
        assert_eq!(result.body, "created");
        assert!(result.latency > Duration::ZERO);
        assert_eq!(mock.calls_async().await, 1);

        Ok(())
    }

    #[tokio::test]
    async fn send_put_with_body_returns_response() -> Result<()> {
        let server = MockServer::start_async().await;

        let mock = server
            .mock_async(|when, then| {
                when.method("PUT").path("/put").body("updated");
                then.status(200).body("ok");
            })
            .await;

        let client = HttpClient::new(Duration::from_secs(10), false, true)?;
        let result = client
            .send(
                "PUT",
                &server.url("/put"),
                Some("updated".to_string()),
                &[],
                RetryConfig::disabled(),
            )
            .await?;

        assert_eq!(result.status, StatusCode::OK);
        assert_eq!(result.body, "ok");
        assert_eq!(mock.calls_async().await, 1);

        Ok(())
    }

    #[tokio::test]
    async fn send_put_without_body_returns_response() -> Result<()> {
        let server = MockServer::start_async().await;

        let mock = server
            .mock_async(|when, then| {
                when.method("PUT").path("/put-no-body").body("");
                then.status(200).body("ok");
            })
            .await;

        let client = HttpClient::new(Duration::from_secs(10), false, true)?;
        let result = client
            .send(
                "PUT",
                &server.url("/put-no-body"),
                None,
                &[],
                RetryConfig::disabled(),
            )
            .await?;

        assert_eq!(result.status, StatusCode::OK);
        assert_eq!(result.body, "ok");
        assert_eq!(mock.calls_async().await, 1);

        Ok(())
    }

    #[tokio::test]
    async fn send_delete_without_body_returns_response() -> Result<()> {
        let server = MockServer::start_async().await;

        let mock = server
            .mock_async(|when, then| {
                when.method("DELETE").path("/delete").body("");
                then.status(204);
            })
            .await;

        let client = HttpClient::new(Duration::from_secs(10), false, true)?;
        let result = client
            .send(
                "DELETE",
                &server.url("/delete"),
                None,
                &[],
                RetryConfig::disabled(),
            )
            .await?;

        assert_eq!(result.status, StatusCode::NO_CONTENT);
        assert_eq!(mock.calls_async().await, 1);

        Ok(())
    }

    #[tokio::test]
    async fn send_patch_sends_body_and_returns_response() -> Result<()> {
        let server = MockServer::start_async().await;

        let mock = server
            .mock_async(|when, then| {
                when.method("PATCH").path("/patch").body("patched");
                then.status(200).body("ok");
            })
            .await;

        let client = HttpClient::new(Duration::from_secs(10), false, true)?;
        let result = client
            .send(
                "PATCH",
                &server.url("/patch"),
                Some("patched".to_string()),
                &[],
                RetryConfig::disabled(),
            )
            .await?;

        assert_eq!(result.status, StatusCode::OK);
        assert_eq!(result.body, "ok");
        assert_eq!(mock.calls_async().await, 1);

        Ok(())
    }

    #[tokio::test]
    async fn send_head_ignores_body() -> Result<()> {
        let server = MockServer::start_async().await;

        let mock = server
            .mock_async(|when, then| {
                when.method("HEAD").path("/head");
                then.status(200);
            })
            .await;

        let client = HttpClient::new(Duration::from_secs(10), false, true)?;
        let result = client
            .send(
                "HEAD",
                &server.url("/head"),
                Some("this-body-must-not-be-sent".to_string()),
                &[],
                RetryConfig::disabled(),
            )
            .await?;

        assert_eq!(result.status, StatusCode::OK);
        assert_eq!(result.body, "");
        assert_eq!(mock.calls_async().await, 1);

        Ok(())
    }

    #[tokio::test]
    async fn send_options_sends_body_and_returns_response() -> Result<()> {
        let server = MockServer::start_async().await;

        let mock = server
            .mock_async(|when, then| {
                when.method("OPTIONS")
                    .path("/options")
                    .body("cors-preflight");
                then.status(204);
            })
            .await;

        let client = HttpClient::new(Duration::from_secs(10), false, true)?;
        let result = client
            .send(
                "OPTIONS",
                &server.url("/options"),
                Some("cors-preflight".to_string()),
                &[],
                RetryConfig::disabled(),
            )
            .await?;

        assert_eq!(result.status, StatusCode::NO_CONTENT);
        assert_eq!(mock.calls_async().await, 1);

        Ok(())
    }

    #[tokio::test]
    async fn send_attaches_custom_headers() -> Result<()> {
        let server = MockServer::start_async().await;

        let mock = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/headers")
                    .header("authorization", "Bearer token123")
                    .header("content-type", "application/json");
                then.status(200).body("ok");
            })
            .await;

        let client = HttpClient::new(Duration::from_secs(10), false, true)?;
        let headers = vec![
            ("Authorization".to_string(), "Bearer token123".to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];

        let result = client
            .send(
                "GET",
                &server.url("/headers"),
                None,
                &headers,
                RetryConfig::disabled(),
            )
            .await?;

        assert_eq!(result.status, StatusCode::OK);
        assert_eq!(mock.calls_async().await, 1);

        Ok(())
    }

    #[tokio::test]
    async fn unsupported_method_returns_error() -> Result<()> {
        let client = HttpClient::new(Duration::from_secs(10), false, true)?;

        let result = client
            .send(
                "TRACE",
                "http://example.test",
                Some("hello".to_string()),
                &[],
                RetryConfig::disabled(),
            )
            .await;

        assert!(result.is_err());

        let error = result.unwrap_err();

        assert_eq!(error.kind(), HttpErrorKind::UnsupportedMethod);
        assert_eq!(error.to_string(), "unsupported method: TRACE");

        Ok(())
    }

    #[test]
    fn new_accepts_insecure_flag() -> Result<()> {
        let client = HttpClient::new(Duration::from_secs(10), true, true);

        assert!(client.is_ok());

        Ok(())
    }

    #[test]
    fn new_defaults_to_keep_alive_when_enabled() -> Result<()> {
        let client = HttpClient::new(Duration::from_secs(10), false, true)?;

        assert!(client.keep_alive());

        Ok(())
    }

    #[test]
    fn new_accepts_no_keep_alive() -> Result<()> {
        let client = HttpClient::new(Duration::from_secs(10), false, false)?;

        assert!(!client.keep_alive());

        Ok(())
    }
}
