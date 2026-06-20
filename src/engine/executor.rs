use crate::ui::{LiveStats, ProgressTracker};
use std::future::Future;
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use regex::Regex;
use reqwest::header::HeaderMap;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::config::RateLimitConfig;
use crate::engine::http_client::{HttpClient, HttpErrorKind, RetryConfig};
use crate::engine::rate_limiter::RateLimiter;
use crate::stats::{ErrorCategory, StatsCollector, StatsSnapshot};

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub url: String,
    pub method: String,
    pub body: Option<String>,
    pub headers: Vec<(String, String)>,
    pub validation: ValidationConfig,
    pub concurrency: usize,
    pub timeout: Duration,
    pub insecure: bool,
    pub rate_limit: Option<RateLimitConfig>,
    pub rate_limiter: Option<Arc<RateLimiter>>,
    pub ramp_up: Option<Duration>,
    pub ramp_up_step: usize,
    pub keep_alive: bool,
    pub retry: usize,
    pub retry_delay: Duration,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidationConfig {
    pub expect_status: Option<Vec<u16>>,
    pub expect_body: Option<String>,
    pub expect_headers: Option<Vec<(String, String)>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidationResult {
    pub passed: bool,
    pub failures: Vec<String>,
}

impl EngineConfig {
    pub fn new(url: impl Into<String>, concurrency: usize) -> Self {
        Self {
            url: url.into(),
            method: "GET".to_string(),
            body: None,
            headers: Vec::new(),
            validation: ValidationConfig::default(),
            concurrency,
            timeout: Duration::from_secs(10),
            insecure: false,
            rate_limiter: None,
            rate_limit: None,
            ramp_up: None,
            ramp_up_step: 1,
            keep_alive: true,
            retry: 0,
            retry_delay: Duration::ZERO,
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
    rate_limit: Option<RateLimitConfig>,
    rate_limiter: Option<Arc<RateLimiter>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShutdownReason {
    Completed,
    Interrupted,
}

type WorkerHandle = JoinHandle<Result<()>>;

#[derive(Debug, Clone, Copy)]
struct RampUpState {
    current_workers: usize,
    target_workers: usize,
    step_interval: Duration,
    last_ramp_up: Instant,
}

impl RampUpState {
    fn new(
        target_workers: usize,
        ramp_up_duration: Duration,
        ramp_up_step: usize,
        now: Instant,
    ) -> Self {
        let step_interval =
            calculate_ramp_up_step_interval(target_workers, ramp_up_duration, ramp_up_step);

        Self {
            current_workers: ramp_up_step.min(target_workers),
            target_workers,
            step_interval,
            last_ramp_up: now,
        }
    }

    fn advance(&mut self, now: Instant, ramp_up_step: usize) -> usize {
        if self.current_workers >= self.target_workers {
            return 0;
        }

        if now.duration_since(self.last_ramp_up) < self.step_interval {
            return 0;
        }

        let remaining_workers = self.target_workers - self.current_workers;
        let added_workers = ramp_up_step.min(remaining_workers);

        self.current_workers += added_workers;
        self.last_ramp_up = now;

        added_workers
    }
}

#[derive(Clone)]
struct RampUpProgress {
    current_workers: Arc<AtomicUsize>,
    target_workers: usize,
}

impl RampUpProgress {
    fn new(initial_workers: usize, target_workers: usize) -> Self {
        Self {
            current_workers: Arc::new(AtomicUsize::new(initial_workers.min(target_workers))),
            target_workers,
        }
    }

    fn set_current_workers(&self, current_workers: usize) {
        self.current_workers
            .store(current_workers.min(self.target_workers), Ordering::SeqCst);
    }

    fn apply_to_live_stats(&self, stats: &mut LiveStats) {
        stats.current_workers = Some(self.current_workers.load(Ordering::SeqCst));
        stats.target_workers = Some(self.target_workers);
    }
}

#[derive(Clone)]
struct WorkerContext {
    shutdown: Arc<AtomicBool>,
    remaining_requests: Option<Arc<AtomicUsize>>,
    stats: Arc<Mutex<StatsCollector>>,
    client: HttpClient,
    method: String,
    url: String,
    body: Option<String>,
    headers: Vec<(String, String)>,
    validation: ValidationConfig,
    retry_config: RetryConfig,
    rate_limiter: Option<Arc<RateLimiter>>,
    progress: ProgressTracker,
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

        if config.ramp_up_step == 0 {
            bail!("ramp-up step must be greater than 0");
        }

        if live_stats_interval.is_zero() {
            bail!("live stats interval must be greater than 0");
        }

        let client = HttpClient::new(config.timeout, config.insecure, config.keep_alive)?;

        let rate_limiter = config.rate_limiter.as_ref().map(Arc::clone);

        let rate_limit = config.rate_limit;

        Ok(Self {
            config,
            client,
            stats: Arc::new(Mutex::new(StatsCollector::new())),
            progress_enabled,
            color_enabled: progress_enabled && color_enabled,
            live_stats_interval,
            rate_limiter,
            rate_limit,
        })
    }

    pub fn stats(&self) -> Arc<Mutex<StatsCollector>> {
        Arc::clone(&self.stats)
    }

    pub fn snapshot(&self) -> StatsSnapshot {
        lock_stats(self.stats.as_ref()).snapshot()
    }

    fn ramp_up_progress(&self) -> Option<RampUpProgress> {
        self.config
            .ramp_up
            .filter(|duration| !duration.is_zero())
            .map(|_| {
                RampUpProgress::new(
                    self.config.ramp_up_step.min(self.config.concurrency),
                    self.config.concurrency,
                )
            })
    }

    fn reset_timer(&self) {
        let mut stats = lock_stats(self.stats.as_ref());
        stats.reset_timer();
    }

    pub async fn run(&self) -> Result<StatsSnapshot> {
        self.reset_timer();

        let progress = ProgressTracker::new_with_color_and_rate_limit(
            None,
            None,
            self.progress_enabled,
            self.color_enabled,
            self.rate_limit,
        );

        self.run_until_shutdown(wait_for_interrupt_signal(), progress)
            .await
    }

    pub async fn run_for_duration(&self, duration: Duration) -> Result<StatsSnapshot> {
        if duration.is_zero() {
            bail!("duration must be greater than 0");
        }

        self.reset_timer();

        let progress = ProgressTracker::new_with_color_and_rate_limit(
            None,
            Some(duration),
            self.progress_enabled,
            self.color_enabled,
            self.rate_limit,
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
        let progress = ProgressTracker::new_with_color_and_rate_limit(
            Some(total_requests as u64),
            None,
            self.progress_enabled,
            self.color_enabled,
            self.rate_limit,
        );

        let ramp_up_progress = self.ramp_up_progress();

        let handles = self.spawn_workers(
            Arc::clone(&shutdown),
            Some(Arc::clone(&remaining_requests)),
            progress.clone(),
            ramp_up_progress.clone(),
        );

        let live_stats_handle = self.spawn_live_stats_updater(
            Arc::clone(&shutdown),
            progress.clone(),
            ramp_up_progress,
        );

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
        let ramp_up_progress = self.ramp_up_progress();

        let live_stats_handle = self.spawn_live_stats_updater(
            Arc::clone(&shutdown),
            progress.clone(),
            ramp_up_progress.clone(),
        );
        let handles = self.spawn_workers(
            Arc::clone(&shutdown),
            None,
            progress.clone(),
            ramp_up_progress,
        );

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
        ramp_up_progress: Option<RampUpProgress>,
    ) -> Vec<WorkerHandle> {
        let worker_context = WorkerContext {
            shutdown,
            remaining_requests,
            stats: Arc::clone(&self.stats),
            client: self.client.clone(),
            method: self.config.method.clone(),
            url: self.config.url.clone(),
            body: self.config.body.clone(),
            headers: self.config.headers.clone(),
            validation: self.config.validation.clone(),
            retry_config: RetryConfig {
                max_retries: self.config.retry,
                delay: self.config.retry_delay,
            },
            rate_limiter: self.rate_limiter.as_ref().map(Arc::clone),
            progress,
        };

        if let Some(ramp_up_duration) = self.config.ramp_up.filter(|duration| !duration.is_zero()) {
            let ramp_up_state = RampUpState::new(
                self.config.concurrency,
                ramp_up_duration,
                self.config.ramp_up_step,
                Instant::now(),
            );

            let ramp_up_progress = ramp_up_progress.unwrap_or_else(|| {
                RampUpProgress::new(ramp_up_state.current_workers, ramp_up_state.target_workers)
            });

            return vec![spawn_ramp_up_workers(
                worker_context,
                ramp_up_state,
                self.config.ramp_up_step,
                ramp_up_progress,
            )];
        }

        spawn_worker_batch(self.config.concurrency, worker_context)
    }

    fn spawn_live_stats_updater(
        &self,
        shutdown: Arc<AtomicBool>,
        progress: ProgressTracker,
        ramp_up_progress: Option<RampUpProgress>,
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
                        live_snapshot_with_ramp_up(&stats, ramp_up_progress.as_ref())
                    };

                    progress.update_live_stats(&live_stats);

                    next_update = now + interval;
                }

                tokio::time::sleep(poll_interval.min(interval)).await;
            }

            let live_stats = {
                let stats = lock_stats(stats.as_ref());
                live_snapshot_with_ramp_up(&stats, ramp_up_progress.as_ref())
            };

            progress.update_live_stats(&live_stats);
        }))
    }
}

fn spawn_ramp_up_workers(
    worker_context: WorkerContext,
    mut ramp_up_state: RampUpState,
    ramp_up_step: usize,
    ramp_up_progress: RampUpProgress,
) -> WorkerHandle {
    tokio::spawn(async move {
        ramp_up_progress.set_current_workers(ramp_up_state.current_workers);
        let mut handles = spawn_worker_batch(ramp_up_state.current_workers, worker_context.clone());

        while ramp_up_state.current_workers < ramp_up_state.target_workers {
            if worker_context.shutdown.load(Ordering::SeqCst)
                || has_no_remaining_requests(&worker_context)
            {
                break;
            }

            if !sleep_or_shutdown(
                ramp_up_state.step_interval,
                worker_context.shutdown.as_ref(),
            )
            .await
            {
                break;
            }

            if has_no_remaining_requests(&worker_context) {
                break;
            }

            let added_workers = ramp_up_state.advance(Instant::now(), ramp_up_step);
            ramp_up_progress.set_current_workers(ramp_up_state.current_workers);

            if added_workers > 0 {
                handles.extend(spawn_worker_batch(added_workers, worker_context.clone()));
            }
        }

        join_worker_handles(handles, "ramp-up worker task failed").await
    })
}

fn spawn_worker_batch(count: usize, worker_context: WorkerContext) -> Vec<WorkerHandle> {
    let mut handles = Vec::with_capacity(count);

    for _ in 0..count {
        handles.push(spawn_worker(worker_context.clone()));
    }

    handles
}

fn spawn_worker(worker_context: WorkerContext) -> WorkerHandle {
    tokio::spawn(async move {
        loop {
            if worker_context.shutdown.load(Ordering::SeqCst) {
                break;
            }

            if let Some(counter) = worker_context.remaining_requests.as_ref() {
                let has_request_slot = counter
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                        current.checked_sub(1)
                    })
                    .is_ok();

                if !has_request_slot {
                    break;
                }
            }

            if let Some(rate_limiter) = worker_context.rate_limiter.as_ref()
                && !acquire_rate_limit(rate_limiter, worker_context.shutdown.as_ref()).await
            {
                break;
            }

            if worker_context.shutdown.load(Ordering::SeqCst) {
                break;
            }

            let result = worker_context
                .client
                .send(
                    &worker_context.method,
                    &worker_context.url,
                    worker_context.body.clone(),
                    &worker_context.headers,
                    worker_context.retry_config,
                )
                .await;

            match result {
                Ok(response) => {
                    let retries = retry_count(response.retry_stats.total_attempts);
                    let validation_result =
                        validate_response(&response, &worker_context.validation);
                    let mut stats = lock_stats(worker_context.stats.as_ref());

                    if validation_result.passed {
                        stats.record_with_retries(
                            response.latency,
                            response.status.as_u16(),
                            retries,
                        );
                    } else {
                        stats.record_error_with_retries(
                            ErrorCategory::Other,
                            validation_result.failures.join("; "),
                            retries,
                        );
                    }
                }
                Err(error) => {
                    let category = error_category(error.kind());
                    let retries = retry_count(error.retry_stats().total_attempts);
                    let mut stats = lock_stats(worker_context.stats.as_ref());

                    stats.record_error_with_retries(category, error, retries);
                }
            }

            worker_context.progress.tick();
        }

        Ok(())
    })
}

fn has_no_remaining_requests(worker_context: &WorkerContext) -> bool {
    worker_context
        .remaining_requests
        .as_ref()
        .is_some_and(|counter| counter.load(Ordering::SeqCst) == 0)
}

async fn sleep_or_shutdown(duration: Duration, shutdown: &AtomicBool) -> bool {
    if duration.is_zero() {
        return !shutdown.load(Ordering::SeqCst);
    }

    let sleep = tokio::time::sleep(duration);
    tokio::pin!(sleep);

    loop {
        if shutdown.load(Ordering::SeqCst) {
            return false;
        }

        tokio::select! {
            _ = &mut sleep => {
                return !shutdown.load(Ordering::SeqCst);
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }
}

fn lock_stats(stats: &Mutex<StatsCollector>) -> MutexGuard<'_, StatsCollector> {
    match stats.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn live_snapshot_with_ramp_up(
    stats: &StatsCollector,
    ramp_up_progress: Option<&RampUpProgress>,
) -> LiveStats {
    let mut live_stats = stats.live_snapshot();

    if let Some(ramp_up_progress) = ramp_up_progress {
        ramp_up_progress.apply_to_live_stats(&mut live_stats);
    }

    live_stats
}

fn calculate_ramp_up_step_count(target_workers: usize, ramp_up_step: usize) -> usize {
    if target_workers == 0 {
        return 0;
    }

    ((target_workers - 1) / ramp_up_step) + 1
}

fn calculate_ramp_up_step_interval(
    target_workers: usize,
    ramp_up_duration: Duration,
    ramp_up_step: usize,
) -> Duration {
    let step_count = calculate_ramp_up_step_count(target_workers, ramp_up_step).max(1);

    ramp_up_duration.div_f64(step_count as f64)
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

fn validate_response(
    response: &crate::engine::http_client::HttpResponse,
    validation: &ValidationConfig,
) -> ValidationResult {
    let mut failures = Vec::new();

    if let Some(expected_statuses) = &validation.expect_status {
        let actual_status = response.status.as_u16();

        if !expected_statuses.contains(&actual_status) {
            failures.push(format!(
                "expected status {:?}, got {}",
                expected_statuses, actual_status
            ));
        }
    }

    if let Some(pattern) = &validation.expect_body {
        match Regex::new(pattern) {
            Ok(regex) => {
                if !regex.is_match(&response.body) {
                    failures.push(format!("expected body to match pattern `{pattern}`"));
                }
            }
            Err(error) => {
                failures.push(format!("invalid body regex `{pattern}`: {error}"));
            }
        }
    }

    if let Some(expected_headers) = &validation.expect_headers {
        for (key, expected_value) in expected_headers {
            match header_value(&response.headers, key) {
                Some(actual_value) if actual_value == expected_value.as_str() => {}
                Some(actual_value) => failures.push(format!(
                    "expected header `{key}: {expected_value}`, got `{key}: {actual_value}`"
                )),
                None => failures.push(format!("expected header `{key}: {expected_value}`")),
            }
        }
    }

    ValidationResult {
        passed: failures.is_empty(),
        failures,
    }
}

fn header_value<'a>(headers: &'a HeaderMap, key: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(name, _)| name.as_str().eq_ignore_ascii_case(key))
        .and_then(|(_, value)| value.to_str().ok())
}

fn retry_count(total_attempts: usize) -> usize {
    total_attempts.saturating_sub(1)
}

async fn acquire_rate_limit(rate_limiter: &RateLimiter, shutdown: &AtomicBool) -> bool {
    loop {
        if shutdown.load(Ordering::SeqCst) {
            return false;
        }

        tokio::select! {
            () = rate_limiter.acquire() => {
                return !shutdown.load(Ordering::SeqCst);
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
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

async fn join_worker_handles(handles: Vec<WorkerHandle>, context: &'static str) -> Result<()> {
    let mut task_error = None;

    for handle in handles {
        match handle.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                let error = error.context(context);
                eprintln!("{error:#}");

                if task_error.is_none() {
                    task_error = Some(error);
                }
            }
            Err(error) => {
                let error = anyhow::Error::new(error).context(context);
                eprintln!("{error:#}");

                if task_error.is_none() {
                    task_error = Some(error);
                }
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
            validation: ValidationConfig::default(),
            concurrency: 10,
            timeout: Duration::from_millis(300),
            insecure: false,
            rate_limit: None,
            rate_limiter: None,
            ramp_up: None,
            ramp_up_step: 1,
            keep_alive: true,
            retry: 0,
            retry_delay: Duration::ZERO,
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
            validation: ValidationConfig::default(),
            concurrency: 10,
            timeout: Duration::from_secs(1),
            insecure: false,
            rate_limit: None,
            rate_limiter: None,
            ramp_up: None,
            ramp_up_step: 1,
            keep_alive: true,
            retry: 0,
            retry_delay: Duration::ZERO,
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
            validation: ValidationConfig::default(),
            concurrency: 10,
            timeout: Duration::from_secs(1),
            insecure: false,
            rate_limit: None,
            rate_limiter: None,
            ramp_up: None,
            ramp_up_step: 1,
            keep_alive: true,
            retry: 0,
            retry_delay: Duration::ZERO,
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
            validation: ValidationConfig::default(),
            concurrency: 5,
            timeout: Duration::from_millis(50),
            insecure: false,
            rate_limit: None,
            rate_limiter: None,
            ramp_up: None,
            ramp_up_step: 1,
            keep_alive: true,
            retry: 0,
            retry_delay: Duration::ZERO,
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
            validation: ValidationConfig::default(),
            concurrency: 1,
            timeout: Duration::from_millis(50),
            insecure: false,
            rate_limit: None,
            rate_limiter: None,
            ramp_up: None,
            ramp_up_step: 1,
            keep_alive: true,
            retry: 0,
            retry_delay: Duration::ZERO,
        };

        let engine = Engine::new_with_progress_and_live_stats_interval(
            config,
            true,
            Duration::from_millis(10),
        )?;

        let shutdown = Arc::new(AtomicBool::new(false));
        let progress = ProgressTracker::new(None, None, false);

        let handle = engine
            .spawn_live_stats_updater(Arc::clone(&shutdown), progress, None)
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
                .spawn_live_stats_updater(shutdown, progress, None)
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
            validation: ValidationConfig::default(),
            concurrency: 5,
            timeout: Duration::from_secs(1),
            insecure: false,
            rate_limit: None,
            rate_limiter: None,
            ramp_up: None,
            ramp_up_step: 1,
            keep_alive: true,
            retry: 0,
            retry_delay: Duration::ZERO,
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
            validation: ValidationConfig::default(),
            concurrency: 2,
            timeout: Duration::from_secs(1),
            insecure: false,
            rate_limit: None,
            rate_limiter: None,
            ramp_up: None,
            ramp_up_step: 1,
            keep_alive: true,
            retry: 0,
            retry_delay: Duration::ZERO,
        };

        let engine = Engine::new(config)?;
        let snapshot = engine.run_for_requests(5).await?;

        assert_eq!(snapshot.total_requests, 5);
        assert_eq!(snapshot.successful_requests, 5);
        assert_eq!(snapshot.total_errors, 0);
        assert_eq!(mock.calls_async().await, 5);

        Ok(())
    }

    #[tokio::test]
    async fn run_for_duration_respects_rate_limit() -> Result<()> {
        let server = MockServer::start_async().await;

        let _mock = server
            .mock_async(|when, then| {
                when.method(GET).path("/rate-limited");
                then.status(200).body("ok");
            })
            .await;

        let config = EngineConfig {
            url: server.url("/rate-limited"),
            method: "GET".to_string(),
            body: None,
            headers: Vec::new(),
            validation: ValidationConfig::default(),
            concurrency: 10,
            timeout: Duration::from_secs(1),
            insecure: false,
            rate_limit: Some(RateLimitConfig {
                rate: 10,
                period: crate::config::RatePeriod::Second,
            }),
            rate_limiter: Some(Arc::new(RateLimiter::new(10, Duration::from_secs(1))?)),
            ramp_up: None,
            ramp_up_step: 1,
            keep_alive: true,
            retry: 0,
            retry_delay: Duration::ZERO,
        };

        let engine = Engine::new(config)?;
        let snapshot = engine.run_for_duration(Duration::from_secs(1)).await?;

        assert!(
            (9..=11).contains(&snapshot.total_requests),
            "expected about 10 requests, got {}",
            snapshot.total_requests
        );

        Ok(())
    }

    #[tokio::test]
    async fn run_for_duration_without_rate_limit_remains_unlimited() -> Result<()> {
        let server = MockServer::start_async().await;

        let _mock = server
            .mock_async(|when, then| {
                when.method(GET).path("/unlimited");
                then.status(200).body("ok");
            })
            .await;

        let config = EngineConfig {
            url: server.url("/unlimited"),
            method: "GET".to_string(),
            body: None,
            headers: Vec::new(),
            validation: ValidationConfig::default(),
            concurrency: 10,
            timeout: Duration::from_secs(1),
            insecure: false,
            rate_limit: None,
            rate_limiter: None,
            ramp_up: None,
            ramp_up_step: 1,
            keep_alive: true,
            retry: 0,
            retry_delay: Duration::ZERO,
        };

        let engine = Engine::new(config)?;
        let snapshot = engine.run_for_duration(Duration::from_millis(200)).await?;

        assert!(
            snapshot.total_requests > 20,
            "expected unlimited mode to send significantly more than 20 requests, got {}",
            snapshot.total_requests
        );

        Ok(())
    }

    #[tokio::test]
    async fn run_for_duration_with_rate_limit_throttles_requests() -> Result<()> {
        let server = MockServer::start_async().await;

        let mock = server
            .mock_async(|when, then| {
                when.method(GET).path("/rate-limited");
                then.status(200).body("ok");
            })
            .await;

        let config = EngineConfig {
            url: server.url("/rate-limited"),
            method: "GET".to_string(),
            body: None,
            headers: Vec::new(),
            validation: ValidationConfig::default(),
            concurrency: 5,
            timeout: Duration::from_secs(1),
            insecure: false,
            rate_limit: Some(RateLimitConfig {
                rate: 10,
                period: crate::config::RatePeriod::Second,
            }),
            rate_limiter: Some(Arc::new(RateLimiter::new(10, Duration::from_secs(1))?)),
            ramp_up: None,
            ramp_up_step: 1,
            keep_alive: true,
            retry: 0,
            retry_delay: Duration::ZERO,
        };

        let engine = Engine::new(config)?;
        let snapshot = engine.run_for_duration(Duration::from_secs(3)).await?;

        assert_eq!(snapshot.total_errors, 0);
        assert_eq!(mock.calls_async().await as u64, snapshot.total_requests);

        assert!(
            (25..=35).contains(&snapshot.total_requests),
            "expected about 30 requests for 10/s over 3s, got {}",
            snapshot.total_requests
        );

        Ok(())
    }

    #[test]
    fn ramp_up_step_interval_uses_one_second_for_ten_workers_over_ten_seconds() {
        assert_eq!(
            calculate_ramp_up_step_interval(10, Duration::from_secs(10), 1),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn ramp_up_step_interval_uses_two_point_five_seconds_for_twenty_workers_step_five() {
        assert_eq!(
            calculate_ramp_up_step_interval(20, Duration::from_secs(10), 5),
            Duration::from_millis(2_500)
        );
    }

    #[test]
    fn ramp_up_state_progresses_by_step_after_interval() {
        let now = Instant::now();
        let mut state = RampUpState::new(10, Duration::from_secs(10), 2, now);

        assert_eq!(state.current_workers, 2);
        assert_eq!(state.target_workers, 10);
        assert_eq!(state.step_interval, Duration::from_secs(2));

        assert_eq!(state.advance(now + Duration::from_secs(1), 2), 0);
        assert_eq!(state.current_workers, 2);

        assert_eq!(state.advance(now + Duration::from_secs(2), 2), 2);
        assert_eq!(state.current_workers, 4);

        assert_eq!(state.advance(now + Duration::from_secs(4), 2), 2);
        assert_eq!(state.current_workers, 6);
    }

    #[tokio::test]
    async fn no_ramp_up_spawns_full_concurrency_immediately() -> Result<()> {
        let config = EngineConfig::new("http://example.com", 3);
        let engine = Engine::new(config)?;
        let shutdown = Arc::new(AtomicBool::new(true));
        let progress = ProgressTracker::new(None, None, false);

        let handles = engine.spawn_workers(shutdown, None, progress, None);

        assert_eq!(handles.len(), 3);

        for handle in handles {
            handle.abort();
        }

        Ok(())
    }

    #[tokio::test]
    async fn zero_ramp_up_is_treated_as_no_ramp_up() -> Result<()> {
        let mut config = EngineConfig::new("http://example.com", 3);
        config.ramp_up = Some(Duration::ZERO);

        let engine = Engine::new(config)?;
        let shutdown = Arc::new(AtomicBool::new(true));
        let progress = ProgressTracker::new(None, None, false);

        let handles = engine.spawn_workers(shutdown, None, progress, None);

        assert_eq!(handles.len(), 3);

        for handle in handles {
            handle.abort();
        }

        Ok(())
    }

    #[test]
    fn ramp_up_step_at_least_concurrency_starts_all_workers_in_single_step() {
        let now = Instant::now();
        let state = RampUpState::new(10, Duration::from_secs(10), 20, now);

        assert_eq!(state.current_workers, 10);
        assert_eq!(state.target_workers, 10);
        assert_eq!(state.step_interval, Duration::from_secs(10));
    }

    #[test]
    fn live_snapshot_with_ramp_up_includes_worker_counts() {
        let stats = StatsCollector::new();
        let ramp_up_progress = RampUpProgress::new(5, 20);

        let live_stats = live_snapshot_with_ramp_up(&stats, Some(&ramp_up_progress));

        assert_eq!(live_stats.current_workers, Some(5));
        assert_eq!(live_stats.target_workers, Some(20));
    }

    #[test]
    fn live_snapshot_without_ramp_up_hides_worker_counts() {
        let stats = StatsCollector::new();

        let live_stats = live_snapshot_with_ramp_up(&stats, None);

        assert_eq!(live_stats.current_workers, None);
        assert_eq!(live_stats.target_workers, None);
    }

    #[test]
    fn ramp_up_progress_applies_worker_counts_to_live_stats() {
        let ramp_up_progress = RampUpProgress::new(3, 10);
        let mut stats = LiveStats::default();

        ramp_up_progress.apply_to_live_stats(&mut stats);

        assert_eq!(stats.current_workers, Some(3));
        assert_eq!(stats.target_workers, Some(10));

        ramp_up_progress.set_current_workers(10);
        ramp_up_progress.apply_to_live_stats(&mut stats);

        assert_eq!(stats.current_workers, Some(10));
        assert_eq!(stats.target_workers, Some(10));
    }

    #[tokio::test]
    async fn join_worker_handles_returns_error_from_worker_result() {
        let handles: Vec<WorkerHandle> = vec![tokio::spawn(async {
            Err::<(), anyhow::Error>(anyhow::anyhow!("worker exploded"))
        })];

        let result = join_worker_handles(handles, "test worker failed").await;

        assert!(result.is_err());

        let error = result.unwrap_err().to_string();

        assert!(
            error.contains("test worker failed"),
            "expected context in error, got: {error}"
        );
    }

    #[tokio::test]
    async fn join_worker_handles_continues_after_worker_error() {
        let completed_workers = Arc::new(AtomicUsize::new(0));

        let completed_workers_for_task = Arc::clone(&completed_workers);

        let handles: Vec<WorkerHandle> = vec![
            tokio::spawn(async {
                Err::<(), anyhow::Error>(anyhow::anyhow!("first worker failed"))
            }),
            tokio::spawn(async move {
                completed_workers_for_task.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }),
        ];

        let result = join_worker_handles(handles, "test worker failed").await;

        assert!(result.is_err());
        assert_eq!(completed_workers.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn engine_config_defaults_to_keep_alive() {
        let config = EngineConfig::new("http://example.com", 1);

        assert!(config.keep_alive);
    }

    #[test]
    fn engine_accepts_no_keep_alive_config() -> Result<()> {
        let mut config = EngineConfig::new("http://example.com", 1);
        config.keep_alive = false;

        let engine = Engine::new(config)?;

        assert!(!engine.client.keep_alive());

        Ok(())
    }

    #[test]
    fn engine_config_defaults_to_retry_disabled() {
        let config = EngineConfig::new("http://example.com", 1);

        assert_eq!(config.retry, 0);
        assert_eq!(config.retry_delay, Duration::ZERO);
    }

    #[tokio::test]
    async fn run_for_requests_records_retry_count_from_5xx_retry() -> Result<()> {
        let server = MockServer::start_async().await;

        let failing = server
            .mock_async(|when, then| {
                when.method(GET).path("/flaky");
                then.status(503).body("service unavailable");
            })
            .await;

        let mut config = EngineConfig::new(server.url("/flaky"), 1);
        config.retry = 2;
        config.retry_delay = Duration::ZERO;

        let engine = Engine::new(config)?;
        let snapshot = engine.run_for_requests(1).await?;

        assert_eq!(snapshot.total_requests, 1);
        assert_eq!(snapshot.total_errors, 1);
        assert_eq!(snapshot.error_counts.http_5xx, 1);
        assert_eq!(snapshot.retries, 2);
        assert_eq!(failing.calls_async().await, 3);

        Ok(())
    }
}
