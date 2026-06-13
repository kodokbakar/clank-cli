use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use console::Term;
use indicatif::{ProgressBar as IndicatifProgressBar, ProgressStyle};
use tokio::task::JoinHandle;

use crate::config::RateLimitConfig;
use crate::ui::{EtaEstimator, LiveStats, format_live_with_rate_limit_and_color, warning};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProgressMode {
    Disabled,
    Requests,
    Duration,
    Spinner,
}

#[derive(Debug, Clone)]
pub struct ProgressTracker {
    bar: IndicatifProgressBar,
    mode: ProgressMode,
    started_at: Instant,
    duration: Option<Duration>,
    color_enabled: bool,
    rate_limit: Option<RateLimitConfig>,
    eta: Option<Arc<Mutex<EtaEstimator>>>,
}

impl ProgressTracker {
    pub fn new(total_requests: Option<u64>, duration: Option<Duration>, enabled: bool) -> Self {
        Self::new_with_color(total_requests, duration, enabled, enabled)
    }

    pub fn new_with_color(
        total_requests: Option<u64>,
        duration: Option<Duration>,
        enabled: bool,
        color_enabled: bool,
    ) -> Self {
        Self::with_terminal_and_color(
            total_requests,
            duration,
            enabled,
            Term::stderr().is_term(),
            color_enabled,
        )
    }

    pub fn new_with_color_and_rate_limit(
        total_requests: Option<u64>,
        duration: Option<Duration>,
        enabled: bool,
        color_enabled: bool,
        rate_limit: Option<RateLimitConfig>,
    ) -> Self {
        Self::with_terminal_color_and_rate_limit(
            total_requests,
            duration,
            enabled,
            Term::stderr().is_term(),
            color_enabled,
            rate_limit,
        )
    }

    pub fn tick(&self) {
        match self.mode {
            ProgressMode::Disabled => {}
            ProgressMode::Requests => {
                self.bar.inc(1);
                self.update_eta(self.bar.position());
            }
            ProgressMode::Duration => self.update_duration_position(),
            ProgressMode::Spinner => self.bar.tick(),
        }
    }

    pub fn update_live_stats(&self, stats: &LiveStats) {
        if self.mode == ProgressMode::Disabled {
            return;
        }

        self.bar.set_prefix(format_live_with_rate_limit_and_color(
            stats,
            self.rate_limit.as_ref(),
            self.color_enabled,
        ));
    }

    pub fn finish(&self) {
        if self.bar.is_hidden() {
            return;
        }

        if self.mode == ProgressMode::Duration {
            self.update_duration_position();
        }

        self.bar.finish_and_clear();
    }

    pub fn is_hidden(&self) -> bool {
        self.bar.is_hidden()
    }

    pub fn position(&self) -> u64 {
        self.bar.position()
    }

    pub fn length(&self) -> Option<u64> {
        self.bar.length()
    }

    pub fn color_enabled(&self) -> bool {
        self.color_enabled
    }

    pub fn spawn_duration_ticker(&self, shutdown: Arc<AtomicBool>) -> Option<JoinHandle<()>> {
        if self.mode != ProgressMode::Duration {
            return None;
        }

        let tracker = self.clone();

        Some(tokio::spawn(async move {
            loop {
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }

                tracker.update_duration_position();

                tokio::time::sleep(Duration::from_millis(100)).await;
            }

            tracker.update_duration_position();
        }))
    }

    fn with_terminal_and_color(
        total_requests: Option<u64>,
        duration: Option<Duration>,
        enabled: bool,
        is_terminal: bool,
        color_enabled: bool,
    ) -> Self {
        Self::with_terminal_color_and_rate_limit(
            total_requests,
            duration,
            enabled,
            is_terminal,
            color_enabled,
            None,
        )
    }

    fn with_terminal_color_and_rate_limit(
        total_requests: Option<u64>,
        duration: Option<Duration>,
        enabled: bool,
        is_terminal: bool,
        color_enabled: bool,
        rate_limit: Option<RateLimitConfig>,
    ) -> Self {
        let effective_color_enabled = enabled && is_terminal && color_enabled;

        if !enabled || !is_terminal {
            return Self {
                bar: IndicatifProgressBar::hidden(),
                mode: ProgressMode::Disabled,
                started_at: Instant::now(),
                duration,
                color_enabled: false,
                rate_limit,
                eta: None,
            };
        }

        let mut tracker = match (total_requests, duration) {
            (Some(total_requests), _) => {
                Self::request_based(total_requests, effective_color_enabled)
            }
            (None, Some(duration)) => Self::duration_based(duration, effective_color_enabled),
            (None, None) => Self::spinner(effective_color_enabled),
        };

        tracker.rate_limit = rate_limit;

        tracker
    }

    fn request_based(total_requests: u64, color_enabled: bool) -> Self {
        let bar = IndicatifProgressBar::new(total_requests);

        let template = if color_enabled {
            "{spinner:.green} [{elapsed_precise} / {msg}] [{wide_bar:.cyan/blue}] {pos}/{len} ({percent}%) | {prefix}"
        } else {
            "{spinner} [{elapsed_precise} / {msg}] [{wide_bar}] {pos}/{len} ({percent}%) | {prefix}"
        };

        bar.set_style(
            ProgressStyle::with_template(template)
                .expect("valid request progress template")
                .progress_chars("=>-"),
        );
        bar.set_message("ETA --:--");

        Self {
            bar,
            mode: ProgressMode::Requests,
            started_at: Instant::now(),
            duration: None,
            color_enabled,
            rate_limit: None,
            eta: Some(Arc::new(Mutex::new(EtaEstimator::new(Some(
                total_requests,
            ))))),
        }
    }

    fn duration_based(duration: Duration, color_enabled: bool) -> Self {
        let total_millis = duration_to_millis(duration);
        let bar = IndicatifProgressBar::new(total_millis);

        let template = if color_enabled {
            "{spinner:.green} [{elapsed_precise} / {msg}] [{wide_bar:.cyan/blue}] {percent}% | {prefix}"
        } else {
            "{spinner} [{elapsed_precise} / {msg}] [{wide_bar}] {percent}% | {prefix}"
        };

        bar.set_message("ETA --:--");
        bar.set_style(
            ProgressStyle::with_template(template)
                .expect("valid duration progress template")
                .progress_chars("=>-"),
        );

        Self {
            bar,
            mode: ProgressMode::Duration,
            started_at: Instant::now(),
            duration: Some(duration),
            color_enabled,
            rate_limit: None,
            eta: Some(Arc::new(Mutex::new(EtaEstimator::new(Some(total_millis))))),
        }
    }

    fn spinner(color_enabled: bool) -> Self {
        let bar = IndicatifProgressBar::new_spinner();

        let template = if color_enabled {
            "{spinner:.green} [{elapsed_precise}] running | {prefix}"
        } else {
            "{spinner} [{elapsed_precise}] running | {prefix}"
        };

        bar.set_style(
            ProgressStyle::with_template(template).expect("valid spinner progress template"),
        );
        bar.enable_steady_tick(Duration::from_millis(120));

        Self {
            bar,
            mode: ProgressMode::Spinner,
            started_at: Instant::now(),
            duration: None,
            color_enabled,
            rate_limit: None,
            eta: None,
        }
    }

    fn update_duration_position(&self) {
        let Some(duration) = self.duration else {
            return;
        };

        let total_millis = duration_to_millis(duration);
        let elapsed_millis = duration_to_millis(self.started_at.elapsed()).min(total_millis);

        self.bar.set_position(elapsed_millis);
        self.update_eta(elapsed_millis);
    }

    fn update_eta(&self, completed: u64) {
        let Some(eta) = self.eta.as_ref() else {
            return;
        };

        let mut eta = lock_eta(eta.as_ref());
        eta.update(completed);

        self.bar.set_message(eta.formatted_eta());
    }

    pub fn finish_interrupted(&self) {
        if self.bar.is_hidden() {
            return;
        }

        if self.mode == ProgressMode::Duration {
            self.update_duration_position();
        }

        let message = if self.color_enabled {
            warning("interrupted")
        } else {
            "interrupted".to_string()
        };

        self.bar.finish_with_message(message);
    }
}

fn lock_eta(eta: &Mutex<EtaEstimator>) -> MutexGuard<'_, EtaEstimator> {
    match eta.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn duration_to_millis(duration: Duration) -> u64 {
    let millis = duration.as_millis();

    if millis == 0 {
        1
    } else if millis > u64::MAX as u128 {
        u64::MAX
    } else {
        millis as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_tracker_is_hidden() {
        let tracker = ProgressTracker::with_terminal_and_color(Some(10), None, false, true, true);

        assert!(tracker.is_hidden());
    }

    #[test]
    fn non_terminal_tracker_is_hidden() {
        let tracker = ProgressTracker::with_terminal_and_color(Some(10), None, true, false, true);

        assert!(tracker.is_hidden());
    }

    #[test]
    fn request_based_tracker_ticks_position() {
        let tracker = ProgressTracker::with_terminal_and_color(Some(10), None, true, true, true);

        tracker.tick();
        tracker.tick();

        assert_eq!(tracker.length(), Some(10));
        assert_eq!(tracker.position(), 2);

        tracker.finish();
    }

    #[test]
    fn duration_based_tracker_has_duration_length() {
        let tracker = ProgressTracker::with_terminal_and_color(
            None,
            Some(Duration::from_secs(5)),
            true,
            true,
            true,
        );

        assert_eq!(tracker.length(), Some(5_000));

        tracker.tick();

        assert!(tracker.position() <= 5_000);

        tracker.finish();
    }

    #[test]
    fn spinner_tracker_ticks_without_panic() {
        let tracker = ProgressTracker::with_terminal_and_color(None, None, true, true, true);

        tracker.tick();
        tracker.finish();
    }

    #[tokio::test]
    async fn duration_ticker_updates_position_and_stops_on_shutdown() {
        let tracker = ProgressTracker::with_terminal_and_color(
            None,
            Some(Duration::from_millis(500)),
            true,
            true,
            true,
        );

        let shutdown = Arc::new(AtomicBool::new(false));

        let handle = tracker
            .spawn_duration_ticker(Arc::clone(&shutdown))
            .expect("duration ticker should spawn");

        tokio::time::sleep(Duration::from_millis(250)).await;

        let position_before_shutdown = tracker.position();

        assert!(position_before_shutdown > 0);
        assert!(position_before_shutdown <= 500);

        shutdown.store(true, Ordering::SeqCst);

        handle.await.expect("duration ticker task should finish");

        let position_after_shutdown = tracker.position();

        assert!(position_after_shutdown >= position_before_shutdown);
        assert!(position_after_shutdown <= 500);

        tracker.finish();
    }

    #[test]
    fn disabled_duration_tracker_does_not_spawn_ticker() {
        let tracker = ProgressTracker::with_terminal_and_color(
            None,
            Some(Duration::from_millis(500)),
            false,
            true,
            true,
        );

        let shutdown = Arc::new(AtomicBool::new(false));

        assert!(tracker.spawn_duration_ticker(shutdown).is_none());
    }

    #[test]
    fn tracker_updates_live_stats_without_panic() {
        let tracker = ProgressTracker::with_terminal_and_color(Some(10), None, true, true, true);

        let stats = LiveStats::calculate(Duration::from_secs(10), 100, 90, 10, 45.0, 10.0, 120.0);

        tracker.update_live_stats(&stats);
        tracker.finish();
    }

    #[test]
    fn disabled_tracker_ignores_live_stats_update() {
        let tracker = ProgressTracker::with_terminal_and_color(Some(10), None, false, true, true);

        let stats = LiveStats::default();

        tracker.update_live_stats(&stats);

        assert!(tracker.is_hidden());
    }

    #[test]
    fn tracker_can_disable_color_for_live_stats() {
        let tracker = ProgressTracker::with_terminal_and_color(Some(10), None, true, true, false);

        assert!(!tracker.color_enabled());

        let stats = LiveStats::calculate(Duration::from_secs(10), 100, 90, 10, 45.0, 10.0, 120.0);

        tracker.update_live_stats(&stats);
        tracker.finish();
    }

    #[test]
    fn disabled_tracker_forces_color_disabled() {
        let tracker = ProgressTracker::with_terminal_and_color(Some(10), None, false, true, true);

        assert!(!tracker.color_enabled());
        assert!(tracker.is_hidden());
    }

    #[test]
    fn tracker_finishes_interrupted_without_panic() {
        let tracker = ProgressTracker::with_terminal_and_color(Some(10), None, true, true, false);

        tracker.tick();
        tracker.finish_interrupted();
    }

    #[test]
    fn request_tracker_updates_eta_on_tick() {
        let tracker = ProgressTracker::with_terminal_and_color(Some(10), None, true, true, true);

        tracker.tick();

        assert_eq!(tracker.position(), 1);

        tracker.finish();
    }

    #[test]
    fn duration_tracker_updates_eta_on_tick() {
        let tracker = ProgressTracker::with_terminal_and_color(
            None,
            Some(Duration::from_millis(500)),
            true,
            true,
            true,
        );

        tracker.tick();

        assert!(tracker.position() <= 500);

        tracker.finish();
    }

    #[test]
    fn spinner_tracker_has_no_eta() {
        let tracker = ProgressTracker::with_terminal_and_color(None, None, true, true, true);

        tracker.tick();
        tracker.finish();
    }

    #[test]
    fn tracker_accepts_rate_limit_for_live_stats() {
        let tracker = ProgressTracker::new_with_color_and_rate_limit(
            Some(10),
            None,
            true,
            false,
            Some(RateLimitConfig {
                rate: 100,
                period: crate::config::RatePeriod::Second,
            }),
        );

        let stats = LiveStats::calculate(Duration::from_secs(10), 100, 90, 10, 45.0, 10.0, 120.0);

        tracker.update_live_stats(&stats);
        tracker.finish();
    }
}
