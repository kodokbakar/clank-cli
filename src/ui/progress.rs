use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use console::Term;
use indicatif::{ProgressBar as IndicatifProgressBar, ProgressStyle};
use tokio::task::JoinHandle;

use crate::ui::{LiveStats, format_live_with_color};

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
}

impl ProgressTracker {
    pub fn new(total_requests: Option<u64>, duration: Option<Duration>, enabled: bool) -> Self {
        Self::new_with_color(total_requests, duration, enabled, true)
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

    pub fn tick(&self) {
        match self.mode {
            ProgressMode::Disabled => {}
            ProgressMode::Requests => self.bar.inc(1),
            ProgressMode::Duration => self.update_duration_position(),
            ProgressMode::Spinner => self.bar.tick(),
        }
    }

    pub fn update_live_stats(&self, stats: &LiveStats) {
        if self.mode == ProgressMode::Disabled {
            return;
        }

        self.bar
            .set_prefix(format_live_with_color(stats, self.color_enabled));
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
        if !enabled || !is_terminal {
            return Self {
                bar: IndicatifProgressBar::hidden(),
                mode: ProgressMode::Disabled,
                started_at: Instant::now(),
                duration,
                color_enabled: false,
            };
        }

        match (total_requests, duration) {
            (Some(total_requests), _) => Self::request_based(total_requests, color_enabled),
            (None, Some(duration)) => Self::duration_based(duration, color_enabled),
            (None, None) => Self::spinner(color_enabled),
        }
    }

    fn request_based(total_requests: u64, color_enabled: bool) -> Self {
        let bar = IndicatifProgressBar::new(total_requests);

        let template = if color_enabled {
            "{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {pos}/{len} ({percent}%) | {prefix}"
        } else {
            "{spinner} [{elapsed_precise}] [{wide_bar}] {pos}/{len} ({percent}%) | {prefix}"
        };

        bar.set_style(
            ProgressStyle::with_template(template)
                .expect("valid request progress template")
                .progress_chars("=>-"),
        );

        Self {
            bar,
            mode: ProgressMode::Requests,
            started_at: Instant::now(),
            duration: None,
            color_enabled,
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

        bar.set_message(format_duration(duration));
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
        }
    }

    fn update_duration_position(&self) {
        let Some(duration) = self.duration else {
            return;
        };

        let total_millis = duration_to_millis(duration);
        let elapsed_millis = duration_to_millis(self.started_at.elapsed()).min(total_millis);

        self.bar.set_position(elapsed_millis);
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

fn format_duration(duration: Duration) -> String {
    let total_secs = duration.as_secs();
    let hours = total_secs / 3_600;
    let minutes = (total_secs % 3_600) / 60;
    let seconds = total_secs % 60;

    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
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

    #[test]
    fn format_duration_formats_seconds_minutes_and_hours() {
        assert_eq!(format_duration(Duration::from_secs(30)), "00:30");
        assert_eq!(format_duration(Duration::from_secs(90)), "01:30");
        assert_eq!(format_duration(Duration::from_secs(3_900)), "01:05:00");
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
}
