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
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: String,
    pub latency: Duration,
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
}

impl HttpClientError {
    pub fn new(kind: HttpErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> HttpErrorKind {
        self.kind
    }
}

impl fmt::Display for HttpClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl Error for HttpClientError {}

impl HttpClient {
    pub fn new(timeout: Duration, insecure: bool) -> Result<Self> {
        let client = Client::builder()
            .timeout(timeout)
            .danger_accept_invalid_certs(insecure)
            .build()
            .context("failed to build HTTP client")?;

        Ok(Self { client })
    }

    pub async fn send(
        &self,
        method: &str,
        url: &str,
        body: Option<String>,
        headers: &[(String, String)],
    ) -> std::result::Result<HttpResponse, HttpClientError> {
        let normalized_method = method.to_ascii_uppercase();

        let request = match normalized_method.as_str() {
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

        let request = apply_headers(request, headers)?;

        let started_at = Instant::now();

        let response = request.send().await.map_err(classify_reqwest_error)?;

        let status = response.status();
        let headers = response.headers().clone();

        let body = response.text().await.map_err(classify_reqwest_error)?;

        let latency = started_at.elapsed();

        Ok(HttpResponse {
            status,
            headers,
            body,
            latency,
        })
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

    use anyhow::Result;
    use httpmock::prelude::*;

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

        let client = HttpClient::new(Duration::from_secs(10), false)?;
        let result = client.send("GET", &server.url("/get"), None, &[]).await?;

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

        let client = HttpClient::new(Duration::from_secs(10), false)?;
        let result = client
            .send("GET", &server.url("/get-no-body"), None, &[])
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

        let client = HttpClient::new(Duration::from_secs(10), false)?;
        let result = client
            .send("POST", &server.url("/post"), Some("hello".to_string()), &[])
            .await?;

        assert_eq!(result.status, StatusCode::CREATED);
        assert_eq!(result.body, "created");
        assert!(result.latency > Duration::ZERO);
        assert_eq!(mock.calls_async().await, 1);

        Ok(())
    }

    #[tokio::test]
    async fn send_put_sends_body_and_returns_response() -> Result<()> {
        let server = MockServer::start_async().await;

        let mock = server
            .mock_async(|when, then| {
                when.method(PUT).path("/put").body("updated");
                then.status(200).body("ok");
            })
            .await;

        let client = HttpClient::new(Duration::from_secs(10), false)?;
        let result = client
            .send("PUT", &server.url("/put"), Some("updated".to_string()), &[])
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

        let client = HttpClient::new(Duration::from_secs(10), false)?;
        let result = client
            .send("DELETE", &server.url("/delete"), None, &[])
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

        let client = HttpClient::new(Duration::from_secs(10), false)?;
        let result = client
            .send(
                "PATCH",
                &server.url("/patch"),
                Some("patched".to_string()),
                &[],
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

        let client = HttpClient::new(Duration::from_secs(10), false)?;
        let result = client
            .send(
                "HEAD",
                &server.url("/head"),
                Some("this-body-must-not-be-sent".to_string()),
                &[],
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

        let client = HttpClient::new(Duration::from_secs(10), false)?;
        let result = client
            .send(
                "OPTIONS",
                &server.url("/options"),
                Some("cors-preflight".to_string()),
                &[],
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

        let client = HttpClient::new(Duration::from_secs(10), false)?;
        let headers = vec![
            ("Authorization".to_string(), "Bearer token123".to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];

        let result = client
            .send("GET", &server.url("/headers"), None, &headers)
            .await?;

        assert_eq!(result.status, StatusCode::OK);
        assert_eq!(mock.calls_async().await, 1);

        Ok(())
    }

    #[tokio::test]
    async fn unsupported_method_returns_error() -> Result<()> {
        let client = HttpClient::new(Duration::from_secs(10), false)?;

        let result = client
            .send(
                "TRACE",
                "http://example.test",
                Some("hello".to_string()),
                &[],
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
        let client = HttpClient::new(Duration::from_secs(10), true);

        assert!(client.is_ok());

        Ok(())
    }
}
