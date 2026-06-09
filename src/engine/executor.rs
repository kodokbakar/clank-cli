use crate::ui::ProgressTracker;
use std::future::Future;
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

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
    pub headers: Vec<(String, String)>,
    pub concurrency: usize,
    pub timeout: Duration,
    pub insecure: bool,
}

impl EngineConfig {
    pub fn new(url: impl Into<String>, concurrency: usize) -> Self {
        Self {
            url: url.into(),
            method: "GET".to_string(),
            body: None,
            headers: Vec::new(),
            concurrency,
            timeout: Duration::from_secs(10),
            insecure: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Engine {
    config: EngineConfig,
    client: HttpClient,
    stats: Arc<Mutex<StatsCollector>>,
    progress_enabled: bool,
    color_enabled: bool,
    live_stats_interval: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShutdownReason {
    Completed,
    Interrupted,
}

impl Engine {
    pub fn new(config: EngineConfig) -> Result<Self> {
        Self::new_with_progress(config, false)
    }

    pub fn new_with_progress(config: EngineConfig, progress_enabled: bool) -> Result<Self> {
        Self::new_with_progress_and_live_stats_interval(
            config,
            progress_enabled,
            Duration::from_secs(1),
        )
    }

    pub fn new_with_progress_and_live_stats_interval(
        config: EngineConfig,
        progress_enabled: bool,
        live_stats_interval: Duration,
    ) -> Result<Self> {
        Self::new_with_progress_color_and_live_stats_interval(
            config,
            progress_enabled,
            progress_enabled,
            live_stats_interval,
        )
    }

    pub fn new_with_progress_color_and_live_stats_interval(
        config: EngineConfig,
        progress_enabled: bool,
        color_enabled: bool,
        live_stats_interval: Duration,
    ) -> Result<Self> {
        if config.url.trim().is_empty() {
            bail!("target URL cannot be empty");
        }

        if config.concurrency == 0 {
            bail!("concurrency must be greater than 0");
        }

        if live_stats_interval.is_zero() {
            bail!("live stats interval must be greater than 0");
        }

        let client = HttpClient::new(config.timeout, config.insecure)?;

        Ok(Self {
            config,
            client,
            stats: Arc::new(Mutex::new(StatsCollector::new())),
            progress_enabled,
            color_enabled: progress_enabled && color_enabled,
            live_stats_interval,
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

        let progress =
            ProgressTracker::new_with_color(None, None, self.progress_enabled, self.color_enabled);

        self.run_until_shutdown(wait_for_interrupt_signal(), progress)
            .await
    }

    pub async fn run_for_duration(&self, duration: Duration) -> Result<StatsSnapshot> {
        if duration.is_zero() {
            bail!("duration must be greater than 0");
        }

        self.reset_timer();

        let progress = ProgressTracker::new_with_color(
            None,
            Some(duration),
            self.progress_enabled,
            self.color_enabled,
        );

        self.run_until_shutdown(
            async move {
                tokio::select! {
                    result = wait_for_interrupt_signal() => {
                        result
                    }
                    _ = timeout(duration, std::future::pending::<()>()) => {
                        Ok(ShutdownReason::Completed)
                    }
                }
            },
            progress,
        )
        .await
    }

    pub async fn run_for_requests(&self, total_requests: usize) -> Result<StatsSnapshot> {
        self.reset_timer();

        if total_requests == 0 {
            return Ok(self.snapshot());
        }

        let shutdown = Arc::new(AtomicBool::new(false));
        let remaining_requests = Arc::new(AtomicUsize::new(total_requests));
        let progress = ProgressTracker::new_with_color(
            Some(total_requests as u64),
            None,
            self.progress_enabled,
            self.color_enabled,
        );

        let handles = self.spawn_workers(
            Arc::clone(&shutdown),
            Some(Arc::clone(&remaining_requests)),
            progress.clone(),
        );

        let live_stats_handle =
            self.spawn_live_stats_updater(Arc::clone(&shutdown), progress.clone());

        let interrupt_signal = wait_for_interrupt_signal();
        tokio::pin!(interrupt_signal);

        let reason = loop {
            if handles.iter().all(JoinHandle::is_finished) {
                break ShutdownReason::Completed;
            }

            tokio::select! {
                result = &mut interrupt_signal => {
                    break result?;
                }
                _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            }
        };

        let force_quit_handle = if reason == ShutdownReason::Interrupted {
            Some(spawn_force_quit_guard())
        } else {
            None
        };

        shutdown.store(true, Ordering::SeqCst);

        join_worker_handles(handles, "executor task failed").await?;

        if let Some(handle) = live_stats_handle {
            let _ = handle.await;
        }

        if let Some(handle) = force_quit_handle {
            handle.abort();
            let _ = handle.await;
        }

        finish_progress(&progress, reason);

        Ok(self.snapshot())
    }

    async fn run_until_shutdown<S>(
        &self,
        shutdown_signal: S,
        progress: ProgressTracker,
    ) -> Result<StatsSnapshot>
    where
        S: Future<Output = Result<ShutdownReason>>,
    {
        let shutdown = Arc::new(AtomicBool::new(false));
        let progress_ticker = progress.spawn_duration_ticker(Arc::clone(&shutdown));
        let live_stats_handle =
            self.spawn_live_stats_updater(Arc::clone(&shutdown), progress.clone());
        let handles = self.spawn_workers(Arc::clone(&shutdown), None, progress.clone());

        let reason = shutdown_signal.await?;

        let force_quit_handle = if reason == ShutdownReason::Interrupted {
            Some(spawn_force_quit_guard())
        } else {
            None
        };

        shutdown.store(true, Ordering::SeqCst);

        join_worker_handles(handles, "executor task failed while shutting down").await?;

        if let Some(handle) = progress_ticker {
            let _ = handle.await;
        }

        if let Some(handle) = live_stats_handle {
            let _ = handle.await;
        }

        if let Some(handle) = force_quit_handle {
            handle.abort();
            let _ = handle.await;
        }

        finish_progress(&progress, reason);

        Ok(self.snapshot())
    }

    fn spawn_workers(
        &self,
        shutdown: Arc<AtomicBool>,
        remaining_requests: Option<Arc<AtomicUsize>>,
        progress: ProgressTracker,
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
            let headers = self.config.headers.clone();

            let progress = progress.clone();

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

                    let result = client.send(&method, &url, body.clone(), &headers).await;

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

                    progress.tick();
                }
            });

            handles.push(handle);
        }

        handles
    }

    fn spawn_live_stats_updater(
        &self,
        shutdown: Arc<AtomicBool>,
        progress: ProgressTracker,
    ) -> Option<JoinHandle<()>> {
        if !self.progress_enabled {
            return None;
        }

        let stats = Arc::clone(&self.stats);
        let interval = self.live_stats_interval;

        Some(tokio::spawn(async move {
            let poll_interval = Duration::from_millis(100);
            let mut next_update = Instant::now();

            loop {
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }

                let now = Instant::now();

                if now >= next_update {
                    let live_stats = {
                        let stats = lock_stats(stats.as_ref());
                        stats.live_snapshot()
                    };

                    progress.update_live_stats(&live_stats);

                    next_update = now + interval;
                }

                tokio::time::sleep(poll_interval.min(interval)).await;
            }

            let live_stats = {
                let stats = lock_stats(stats.as_ref());
                stats.live_snapshot()
            };

            progress.update_live_stats(&live_stats);
        }))
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

async fn wait_for_interrupt_signal() -> Result<ShutdownReason> {
    tokio::signal::ctrl_c()
        .await
        .context("failed to listen for Ctrl+C signal")?;

    eprintln!("Shutting down... (press Ctrl+C again to force quit)");

    Ok(ShutdownReason::Interrupted)
}

fn spawn_force_quit_guard() -> JoinHandle<()> {
    tokio::spawn(async {
        let second_signal =
            tokio::time::timeout(force_quit_window(), tokio::signal::ctrl_c()).await;

        if let Ok(Ok(())) = second_signal {
            eprintln!("Force quit.");
            std::process::exit(130);
        }
    })
}

fn force_quit_window() -> Duration {
    Duration::from_secs(2)
}

async fn join_worker_handles(handles: Vec<JoinHandle<()>>, context: &'static str) -> Result<()> {
    let mut task_error = None;

    for handle in handles {
        if let Err(error) = handle.await.context(context) {
            if task_error.is_none() {
                task_error = Some(error);
            }
        }
    }

    if let Some(error) = task_error {
        return Err(error);
    }

    Ok(())
}

fn finish_progress(progress: &ProgressTracker, reason: ShutdownReason) {
    match reason {
        ShutdownReason::Completed => progress.finish(),
        ShutdownReason::Interrupted => progress.finish_interrupted(),
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
            headers: Vec::new(),
            concurrency: 10,
            timeout: Duration::from_millis(300),
            insecure: false,
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
            headers: Vec::new(),
            concurrency: 10,
            timeout: Duration::from_secs(1),
            insecure: false,
        };

        let engine = Engine::new(config)?;
        let progress = ProgressTracker::new(None, None, false);

        let snapshot = engine
            .run_until_shutdown(
                async {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    Ok(ShutdownReason::Interrupted)
                },
                progress,
            )
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
            headers: Vec::new(),
            concurrency: 10,
            timeout: Duration::from_secs(1),
            insecure: false,
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

        let _mock = server
            .mock_async(|when, then| {
                when.method(GET).path("/slow");
                then.status(200)
                    .delay(Duration::from_millis(200))
                    .body("slow response");
            })
            .await;

        let config = EngineConfig {
            url: server.url("/slow"),
            method: "GET".to_string(),
            body: None,
            headers: Vec::new(),
            concurrency: 5,
            timeout: Duration::from_millis(50),
            insecure: false,
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
        assert_eq!(snapshot.error_counts.other, 0);

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

    #[test]
    fn zero_live_stats_interval_returns_error() {
        let config = EngineConfig::new("http://example.com", 1);

        let result =
            Engine::new_with_progress_and_live_stats_interval(config, true, Duration::ZERO);

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn live_stats_updater_stops_without_leaking_task() -> Result<()> {
        let config = EngineConfig {
            url: "http://127.0.0.1:1/unreachable".to_string(),
            method: "GET".to_string(),
            body: None,
            headers: Vec::new(),
            concurrency: 1,
            timeout: Duration::from_millis(50),
            insecure: false,
        };

        let engine = Engine::new_with_progress_and_live_stats_interval(
            config,
            true,
            Duration::from_millis(10),
        )?;

        let shutdown = Arc::new(AtomicBool::new(false));
        let progress = ProgressTracker::new(None, None, false);

        let handle = engine
            .spawn_live_stats_updater(Arc::clone(&shutdown), progress)
            .expect("live stats updater should spawn when progress is enabled");

        tokio::time::sleep(Duration::from_millis(30)).await;

        shutdown.store(true, Ordering::SeqCst);

        handle.await.context("live stats updater task failed")?;

        Ok(())
    }

    #[test]
    fn live_stats_updater_does_not_spawn_when_progress_disabled() -> Result<()> {
        let config = EngineConfig::new("http://example.com", 1);
        let engine = Engine::new_with_progress(config, false)?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let progress = ProgressTracker::new(None, None, false);

        assert!(
            engine
                .spawn_live_stats_updater(shutdown, progress)
                .is_none()
        );

        Ok(())
    }

    #[test]
    fn force_quit_window_is_two_seconds() {
        assert_eq!(force_quit_window(), Duration::from_secs(2));
    }

    #[tokio::test]
    async fn run_until_shutdown_returns_final_snapshot_after_interrupt() -> Result<()> {
        let server = MockServer::start_async().await;

        let mock = server
            .mock_async(|when, then| {
                when.method(GET).path("/interrupt");
                then.status(200).body("ok");
            })
            .await;

        let config = EngineConfig {
            url: server.url("/interrupt"),
            method: "GET".to_string(),
            body: None,
            headers: Vec::new(),
            concurrency: 5,
            timeout: Duration::from_secs(1),
            insecure: false,
        };

        let engine = Engine::new(config)?;
        let progress = ProgressTracker::new(None, None, false);

        let snapshot = engine
            .run_until_shutdown(
                async {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    Ok(ShutdownReason::Interrupted)
                },
                progress,
            )
            .await?;

        assert_eq!(snapshot.total_errors, 0);
        assert!(snapshot.total_requests > 0);
        assert_eq!(mock.calls_async().await as u64, snapshot.total_requests);

        Ok(())
    }

    #[tokio::test]
    async fn run_for_requests_returns_final_snapshot_after_completed_workers() -> Result<()> {
        let server = MockServer::start_async().await;

        let mock = server
            .mock_async(|when, then| {
                when.method(GET).path("/completed");
                then.status(200).body("ok");
            })
            .await;

        let config = EngineConfig::new(server.url("/completed"), 5);
        let engine = Engine::new(config)?;

        let snapshot = engine.run_for_requests(20).await?;

        assert_eq!(snapshot.total_requests, 20);
        assert_eq!(snapshot.successful_requests, 20);
        assert_eq!(snapshot.total_errors, 0);
        assert_eq!(mock.calls_async().await, 20);

        Ok(())
    }

    #[tokio::test]
    async fn run_for_requests_sends_custom_headers() -> Result<()> {
        let server = MockServer::start_async().await;

        let mock = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/headers")
                    .header("authorization", "Bearer token123");
                then.status(200).body("ok");
            })
            .await;

        let config = EngineConfig {
            url: server.url("/headers"),
            method: "GET".to_string(),
            body: None,
            headers: vec![("Authorization".to_string(), "Bearer token123".to_string())],
            concurrency: 2,
            timeout: Duration::from_secs(1),
            insecure: false,
        };

        let engine = Engine::new(config)?;
        let snapshot = engine.run_for_requests(5).await?;

        assert_eq!(snapshot.total_requests, 5);
        assert_eq!(snapshot.successful_requests, 5);
        assert_eq!(snapshot.total_errors, 0);
        assert_eq!(mock.calls_async().await, 5);

        Ok(())
    }
}
