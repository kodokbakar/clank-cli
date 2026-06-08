use std::future::Future;
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::engine::http_client::{HttpClient, HttpErrorKind};
use crate::stats::{ErrorCategory, StatsCollector, StatsSnapshot};

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub url: String,
    pub method: String,
    pub body: Option<String>,
    pub concurrency: usize,
    pub timeout: Duration,
}

impl EngineConfig {
    pub fn new(url: impl Into<String>, concurrency: usize) -> Self {
        Self {
            url: url.into(),
            method: "GET".to_string(),
            body: None,
            concurrency,
            timeout: Duration::from_secs(10),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Engine {
    config: EngineConfig,
    client: HttpClient,
    stats: Arc<Mutex<StatsCollector>>,
}

impl Engine {
    pub fn new(config: EngineConfig) -> Result<Self> {
        if config.url.trim().is_empty() {
            bail!("target URL cannot be empty");
        }

        if config.concurrency == 0 {
            bail!("concurrency must be greater than 0");
        }

        let client = HttpClient::new(config.timeout)?;

        Ok(Self {
            config,
            client,
            stats: Arc::new(Mutex::new(StatsCollector::new())),
        })
    }

    pub fn stats(&self) -> Arc<Mutex<StatsCollector>> {
        Arc::clone(&self.stats)
    }

    pub fn snapshot(&self) -> StatsSnapshot {
        lock_stats(self.stats.as_ref()).snapshot()
    }

    fn reset_timer(&self) {
        let mut stats = lock_stats(self.stats.as_ref());
        stats.reset_timer();
    }

    pub async fn run(&self) -> Result<StatsSnapshot> {
        self.reset_timer();

        self.run_until_shutdown(async {
            tokio::signal::ctrl_c()
                .await
                .context("failed to listen for Ctrl+C signal")
        })
        .await
    }

    pub async fn run_for_duration(&self, duration: Duration) -> Result<StatsSnapshot> {
        if duration.is_zero() {
            bail!("duration must be greater than 0");
        }

        self.reset_timer();

        self.run_until_shutdown(async move {
            tokio::select! {
                result = tokio::signal::ctrl_c() => {
                    result.context("failed to listen for Ctrl+C signal")
                }
                _ = timeout(duration, std::future::pending::<()>()) => {
                    Ok(())
                }
            }
        })
        .await
    }

    pub async fn run_for_requests(&self, total_requests: usize) -> Result<StatsSnapshot> {
        self.reset_timer();

        if total_requests == 0 {
            return Ok(self.snapshot());
        }

        let shutdown = Arc::new(AtomicBool::new(false));
        let remaining_requests = Arc::new(AtomicUsize::new(total_requests));
        let handles =
            self.spawn_workers(Arc::clone(&shutdown), Some(Arc::clone(&remaining_requests)));

        for handle in handles {
            handle.await.context("executor task failed")?;
        }

        Ok(self.snapshot())
    }

    async fn run_until_shutdown<S>(&self, shutdown_signal: S) -> Result<StatsSnapshot>
    where
        S: Future<Output = Result<()>>,
    {
        let shutdown = Arc::new(AtomicBool::new(false));
        let handles = self.spawn_workers(Arc::clone(&shutdown), None);

        let signal_result = shutdown_signal.await;

        shutdown.store(true, Ordering::SeqCst);

        for handle in handles {
            handle
                .await
                .context("executor task failed while shutting down")?;
        }

        signal_result?;

        Ok(self.snapshot())
    }

    fn spawn_workers(
        &self,
        shutdown: Arc<AtomicBool>,
        remaining_requests: Option<Arc<AtomicUsize>>,
    ) -> Vec<JoinHandle<()>> {
        let mut handles = Vec::with_capacity(self.config.concurrency);

        for _ in 0..self.config.concurrency {
            let shutdown = Arc::clone(&shutdown);
            let remaining_requests = remaining_requests.as_ref().map(Arc::clone);
            let stats = Arc::clone(&self.stats);
            let client = self.client.clone();
            let method = self.config.method.clone();
            let url = self.config.url.clone();
            let body = self.config.body.clone();

            let handle = tokio::spawn(async move {
                loop {
                    if shutdown.load(Ordering::SeqCst) {
                        break;
                    }

                    if let Some(counter) = remaining_requests.as_ref() {
                        let has_request_slot = counter
                            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                                current.checked_sub(1)
                            })
                            .is_ok();

                        if !has_request_slot {
                            break;
                        }
                    }

                    let result = client.send(&method, &url, body.clone()).await;

                    match result {
                        Ok(response) => {
                            let mut stats = lock_stats(stats.as_ref());
                            stats.record(response.latency, response.status.as_u16());
                        }
                        Err(error) => {
                            let category = error_category(error.kind());
                            let mut stats = lock_stats(stats.as_ref());

                            stats.record_error(category, error);
                        }
                    }
                }
            });

            handles.push(handle);
        }

        handles
    }
}

fn lock_stats(stats: &Mutex<StatsCollector>) -> MutexGuard<'_, StatsCollector> {
    match stats.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn error_category(kind: HttpErrorKind) -> ErrorCategory {
    match kind {
        HttpErrorKind::Timeout => ErrorCategory::Timeout,
        HttpErrorKind::Connection => ErrorCategory::Connection,
        HttpErrorKind::Request | HttpErrorKind::UnsupportedMethod | HttpErrorKind::Other => {
            ErrorCategory::Other
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use httpmock::prelude::*;

    #[tokio::test]
    async fn run_for_requests_records_all_successful_requests() -> Result<()> {
        let server = MockServer::start_async().await;

        let mock = server
            .mock_async(|when, then| {
                when.method(GET).path("/load");
                then.status(200).body("ok");
            })
            .await;

        let config = EngineConfig::new(server.url("/load"), 10);
        let engine = Engine::new(config)?;

        let snapshot = engine.run_for_requests(10).await?;

        assert_eq!(snapshot.total_requests, 10);
        assert_eq!(snapshot.total_errors, 0);
        assert_eq!(snapshot.status_codes.get(&200), Some(&10));
        assert_eq!(snapshot.latencies.len(), 10);
        assert_eq!(mock.calls_async().await, 10);

        Ok(())
    }

    #[tokio::test]
    async fn run_for_requests_records_errors_without_panic() -> Result<()> {
        let config = EngineConfig {
            url: "http://127.0.0.1:1/unreachable".to_string(),
            method: "GET".to_string(),
            body: None,
            concurrency: 10,
            timeout: Duration::from_millis(300),
        };

        let engine = Engine::new(config)?;
        let snapshot = engine.run_for_requests(10).await?;

        assert_eq!(snapshot.total_requests, 10);
        assert_eq!(snapshot.successful_requests, 0);
        assert_eq!(snapshot.total_errors, 10);
        assert_eq!(snapshot.error_counts.connection, 10);
        assert_eq!(snapshot.error_counts.timeout, 0);
        assert_eq!(snapshot.error_counts.http_4xx, 0);
        assert_eq!(snapshot.error_counts.http_5xx, 0);
        assert_eq!(snapshot.error_counts.other, 0);

        Ok(())
    }

    #[tokio::test]
    async fn run_until_shutdown_stops_workers_without_panic() -> Result<()> {
        let server = MockServer::start_async().await;

        let mock = server
            .mock_async(|when, then| {
                when.method(GET).path("/shutdown");
                then.status(200).body("ok");
            })
            .await;

        let config = EngineConfig {
            url: server.url("/shutdown"),
            method: "GET".to_string(),
            body: None,
            concurrency: 10,
            timeout: Duration::from_secs(1),
        };

        let engine = Engine::new(config)?;

        let snapshot = engine
            .run_until_shutdown(async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                Ok(())
            })
            .await?;

        assert_eq!(snapshot.total_errors, 0);
        assert!(snapshot.total_requests > 0);
        assert_eq!(mock.calls_async().await as u64, snapshot.total_requests);

        Ok(())
    }

    #[tokio::test]
    async fn zero_concurrency_returns_error() {
        let config = EngineConfig::new("http://example.com", 0);
        let result = Engine::new(config);

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn run_for_duration_stops_workers_without_panic() -> Result<()> {
        let server = MockServer::start_async().await;

        let mock = server
            .mock_async(|when, then| {
                when.method(GET).path("/duration");
                then.status(200).body("ok");
            })
            .await;

        let config = EngineConfig {
            url: server.url("/duration"),
            method: "GET".to_string(),
            body: None,
            concurrency: 10,
            timeout: Duration::from_secs(1),
        };

        let engine = Engine::new(config)?;
        let snapshot = engine.run_for_duration(Duration::from_millis(50)).await?;

        assert_eq!(snapshot.total_errors, 0);
        assert!(snapshot.total_requests > 0);
        assert_eq!(mock.calls_async().await as u64, snapshot.total_requests);

        Ok(())
    }

    #[tokio::test]
    async fn run_for_requests_records_4xx_as_http_client_error() -> Result<()> {
        let server = MockServer::start_async().await;

        let mock = server
            .mock_async(|when, then| {
                when.method(GET).path("/not-found");
                then.status(404).body("not found");
            })
            .await;

        let config = EngineConfig::new(server.url("/not-found"), 10);
        let engine = Engine::new(config)?;

        let snapshot = engine.run_for_requests(10).await?;

        assert_eq!(snapshot.total_requests, 10);
        assert_eq!(snapshot.successful_requests, 0);
        assert_eq!(snapshot.total_errors, 10);
        assert_eq!(snapshot.error_counts.http_4xx, 10);
        assert_eq!(snapshot.error_counts.http_5xx, 0);
        assert_eq!(snapshot.error_counts.connection, 0);
        assert_eq!(snapshot.error_counts.timeout, 0);
        assert_eq!(mock.calls_async().await, 10);

        Ok(())
    }

    #[tokio::test]
    async fn run_for_requests_records_5xx_as_http_server_error() -> Result<()> {
        let server = MockServer::start_async().await;

        let mock = server
            .mock_async(|when, then| {
                when.method(GET).path("/server-error");
                then.status(503).body("service unavailable");
            })
            .await;

        let config = EngineConfig::new(server.url("/server-error"), 10);
        let engine = Engine::new(config)?;

        let snapshot = engine.run_for_requests(10).await?;

        assert_eq!(snapshot.total_requests, 10);
        assert_eq!(snapshot.successful_requests, 0);
        assert_eq!(snapshot.total_errors, 10);
        assert_eq!(snapshot.error_counts.http_4xx, 0);
        assert_eq!(snapshot.error_counts.http_5xx, 10);
        assert_eq!(snapshot.error_counts.connection, 0);
        assert_eq!(snapshot.error_counts.timeout, 0);
        assert_eq!(mock.calls_async().await, 10);

        Ok(())
    }

    #[tokio::test]
    async fn run_for_requests_records_timeout_errors() -> Result<()> {
        let server = MockServer::start_async().await;

        let mock = server
            .mock_async(|when, then| {
                when.method(GET).path("/slow");
                then.status(200)
                    .delay(Duration::from_millis(100))
                    .body("slow response");
            })
            .await;

        let config = EngineConfig {
            url: server.url("/slow"),
            method: "GET".to_string(),
            body: None,
            concurrency: 5,
            timeout: Duration::from_millis(10),
        };

        let engine = Engine::new(config)?;
        let snapshot = engine.run_for_requests(5).await?;

        assert_eq!(snapshot.total_requests, 5);
        assert_eq!(snapshot.successful_requests, 0);
        assert_eq!(snapshot.total_errors, 5);
        assert_eq!(snapshot.error_counts.timeout, 5);
        assert_eq!(snapshot.error_counts.connection, 0);
        assert_eq!(snapshot.error_counts.http_4xx, 0);
        assert_eq!(snapshot.error_counts.http_5xx, 0);
        assert_eq!(mock.calls_async().await, 5);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_stats_recording_is_thread_safe() -> Result<()> {
        let stats = Arc::new(Mutex::new(StatsCollector::new()));
        let mut handles = Vec::new();

        for _ in 0..100 {
            let stats = Arc::clone(&stats);

            let handle = tokio::spawn(async move {
                for _ in 0..100 {
                    let mut stats = lock_stats(stats.as_ref());
                    stats.record(Duration::from_millis(10), 200);
                }
            });

            handles.push(handle);
        }

        for handle in handles {
            handle.await.context("stats recording task panicked")?;
        }

        let snapshot = lock_stats(stats.as_ref()).snapshot();

        assert_eq!(snapshot.total_requests, 10_000);
        assert_eq!(snapshot.successful_requests, 10_000);
        assert_eq!(snapshot.total_errors, 0);
        assert_eq!(snapshot.histogram.len(), 10_000);
        assert_eq!(snapshot.error_counts.total(), 0);

        Ok(())
    }

    #[test]
    fn lock_stats_recovers_from_poisoned_mutex() {
        let stats = Arc::new(Mutex::new(StatsCollector::new()));
        let poisoned_stats = Arc::clone(&stats);

        let result = std::thread::spawn(move || {
            let mut stats = poisoned_stats.lock().unwrap();

            stats.record(Duration::from_millis(10), 200);

            panic!("intentional panic while holding stats lock");
        })
        .join();

        assert!(result.is_err());

        {
            let mut stats = lock_stats(stats.as_ref());
            stats.record(Duration::from_millis(20), 200);
        }

        let snapshot = lock_stats(stats.as_ref()).snapshot();

        assert_eq!(snapshot.total_requests, 2);
        assert_eq!(snapshot.successful_requests, 2);
        assert_eq!(snapshot.total_errors, 0);
        assert_eq!(snapshot.histogram.len(), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_mixed_stats_recording_is_thread_safe() -> Result<()> {
        let stats = Arc::new(Mutex::new(StatsCollector::new()));
        let mut handles = Vec::new();

        for _ in 0..100 {
            let stats = Arc::clone(&stats);

            let handle = tokio::spawn(async move {
                for index in 0..100 {
                    let mut stats = lock_stats(stats.as_ref());

                    match index % 5 {
                        0 => stats.record(Duration::from_millis(10), 200),
                        1 => stats.record(Duration::from_millis(20), 404),
                        2 => stats.record(Duration::from_millis(30), 503),
                        3 => stats.record_error(ErrorCategory::Timeout, "request timed out"),
                        _ => stats.record_error(ErrorCategory::Connection, "connection refused"),
                    }
                }
            });

            handles.push(handle);
        }

        for handle in handles {
            handle
                .await
                .context("mixed stats recording task panicked")?;
        }

        let snapshot = lock_stats(stats.as_ref()).snapshot();

        assert_eq!(snapshot.total_requests, 10_000);
        assert_eq!(snapshot.successful_requests, 2_000);
        assert_eq!(snapshot.total_errors, 8_000);
        assert_eq!(snapshot.error_counts.http_4xx, 2_000);
        assert_eq!(snapshot.error_counts.http_5xx, 2_000);
        assert_eq!(snapshot.error_counts.timeout, 2_000);
        assert_eq!(snapshot.error_counts.connection, 2_000);
        assert_eq!(snapshot.error_counts.other, 0);
        assert_eq!(snapshot.error_counts.total(), 8_000);
        assert_eq!(snapshot.histogram.len(), 6_000);

        Ok(())
    }
}
