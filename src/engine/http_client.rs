use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::{Client, StatusCode, header::HeaderMap};

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

impl HttpClient {
    pub fn new(timeout: Duration) -> Result<Self> {
        let client = Client::builder()
            .timeout(timeout)
            .build()
            .context("failed to build HTTP client")?;

        Ok(Self { client })
    }

    pub async fn send(
        &self,
        method: &str,
        url: &str,
        body: Option<String>,
    ) -> Result<HttpResponse> {
        let normalized_method = method.to_ascii_uppercase();

        let request = match normalized_method.as_str() {
            "GET" => self.client.get(url),
            "POST" => {
                let request = self.client.post(url);

                match body {
                    Some(body) => request.body(body),
                    None => request,
                }
            }
            _ => bail!(
                "unsupported HTTP method: {}. Supported methods: GET, POST",
                method
            ),
        };

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

fn classify_reqwest_error(error: reqwest::Error) -> anyhow::Error {
    if error.is_timeout() {
        anyhow!("request timed out: {}", error)
    } else if error.is_connect() {
        anyhow!(
            "connection failed, DNS failed, or target refused connection: {}",
            error
        )
    } else if error.is_request() {
        anyhow!("invalid request: {}", error)
    } else {
        anyhow!("HTTP client error: {}", error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let client = HttpClient::new(Duration::from_secs(10))?;
        let result = client.send("GET", &server.url("/get"), None).await?;

        assert_eq!(result.status, StatusCode::OK);
        assert!(result.headers.contains_key("content-type"));
        assert_eq!(result.body, r#"{"ok":true}"#);
        assert!(result.latency > Duration::ZERO);

        mock.assert_async().await;

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

        let client = HttpClient::new(Duration::from_secs(10))?;
        let result = client
            .send("POST", &server.url("/post"), Some("hello".to_string()))
            .await?;

        assert_eq!(result.status, StatusCode::CREATED);
        assert_eq!(result.body, "created");
        assert!(result.latency > Duration::ZERO);

        mock.assert_async().await;

        Ok(())
    }

    #[tokio::test]
    async fn unsupported_method_returns_error() -> Result<()> {
        let client = HttpClient::new(Duration::from_secs(10))?;

        let result = client
            .send("PUT", "http://example.test", Some("hello".to_string()))
            .await;

        assert!(result.is_err());

        Ok(())
    }
}
