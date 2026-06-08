use std::future::Future;
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::engine::http_client::HttpClient;
use crate::stats::{StatsCollector, StatsSnapshot};

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

    pub async fn run(&self) -> Result<StatsSnapshot> {
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
                            let mut stats = lock_stats(stats.as_ref());
                            stats.record_error(error);
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

        assert_eq!(snapshot.total_requests, 0);
        assert_eq!(snapshot.total_errors, 10);

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
}
