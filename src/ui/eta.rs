use std::time::{Duration, Instant};

const DEFAULT_ALPHA: f64 = 0.3;

#[derive(Debug, Clone)]
pub struct EtaEstimator {
    start: Instant,
    total: Option<u64>,
    completed: u64,
    last_completed: u64,
    last_update: Instant,
    smoothed_rate: Option<f64>,
    alpha: f64,
}

impl EtaEstimator {
    pub fn new(total: Option<u64>) -> Self {
        Self::new_at(total, Instant::now())
    }

    pub fn update(&mut self, completed: u64) {
        self.update_at(completed, Instant::now());
    }

    pub fn eta(&self) -> Option<Duration> {
        let total = self.total?;

        if self.completed >= total {
            return Some(Duration::ZERO);
        }

        let rate = self.smoothed_rate.or_else(|| {
            let elapsed_secs = self.elapsed().as_secs_f64();

            if elapsed_secs > 0.0 && self.completed > 0 {
                Some(self.completed as f64 / elapsed_secs)
            } else {
                None
            }
        })?;

        if rate <= 0.0 {
            return None;
        }

        let remaining = total - self.completed;

        Some(Duration::from_secs_f64(remaining as f64 / rate))
    }

    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    pub fn percentage(&self) -> f64 {
        let Some(total) = self.total else {
            return 0.0;
        };

        if total == 0 {
            return 100.0;
        }

        (self.completed as f64 / total as f64 * 100.0).clamp(0.0, 100.0)
    }

    pub fn formatted_eta(&self) -> String {
        match self.eta() {
            Some(eta) => format!("ETA {}", format_duration(eta)),
            None => "ETA --:--".to_string(),
        }
    }

    fn new_at(total: Option<u64>, now: Instant) -> Self {
        Self {
            start: now,
            total,
            completed: 0,
            last_completed: 0,
            last_update: now,
            smoothed_rate: None,
            alpha: DEFAULT_ALPHA,
        }
    }

    fn update_at(&mut self, completed: u64, now: Instant) {
        let elapsed_since_last = now.duration_since(self.last_update).as_secs_f64();
        let completed_delta = completed.saturating_sub(self.last_completed);

        if elapsed_since_last > 0.0 && completed_delta > 0 {
            let current_rate = completed_delta as f64 / elapsed_since_last;

            self.smoothed_rate = Some(match self.smoothed_rate {
                Some(previous_rate) => {
                    self.alpha * current_rate + (1.0 - self.alpha) * previous_rate
                }
                None => current_rate,
            });

            self.last_update = now;
            self.last_completed = completed;
        }

        self.completed = completed;
    }

    #[cfg(test)]
    fn smoothed_rate(&self) -> Option<f64> {
        self.smoothed_rate
    }
}

fn format_duration(duration: Duration) -> String {
    let mut seconds = duration.as_secs();

    if duration.subsec_nanos() > 0 {
        seconds += 1;
    }

    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;

    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_duration_close(actual: Option<Duration>, expected: Duration) {
        let actual = actual.expect("eta should exist");
        let diff = actual.abs_diff(expected);

        assert!(
            diff <= Duration::from_millis(10),
            "actual: {:?}, expected: {:?}, diff: {:?}",
            actual,
            expected,
            diff
        );
    }

    #[test]
    fn eta_returns_none_when_no_progress_has_been_recorded() {
        let estimator = EtaEstimator::new(Some(100));

        assert_eq!(estimator.eta(), None);
        assert_eq!(estimator.formatted_eta(), "ETA --:--");
    }

    #[test]
    fn eta_returns_none_when_total_is_unknown() {
        let start = Instant::now();
        let mut estimator = EtaEstimator::new_at(None, start);

        estimator.update_at(50, start + Duration::from_secs(5));

        assert_eq!(estimator.eta(), None);
        assert_eq!(estimator.percentage(), 0.0);
    }

    #[test]
    fn eta_calculates_remaining_time_from_rate() {
        let start = Instant::now();
        let mut estimator = EtaEstimator::new_at(Some(100), start);

        estimator.update_at(50, start + Duration::from_secs(10));

        assert_duration_close(estimator.eta(), Duration::from_secs(10));
        assert_eq!(estimator.formatted_eta(), "ETA 00:10");
        assert!((estimator.percentage() - 50.0).abs() < 0.0001);
    }

    #[test]
    fn eta_returns_zero_when_completed_reaches_total() {
        let start = Instant::now();
        let mut estimator = EtaEstimator::new_at(Some(100), start);

        estimator.update_at(100, start + Duration::from_secs(10));

        assert_eq!(estimator.eta(), Some(Duration::ZERO));
        assert_eq!(estimator.formatted_eta(), "ETA 00:00");
        assert_eq!(estimator.percentage(), 100.0);
    }

    #[test]
    fn eta_handles_single_request_after_start() {
        let start = Instant::now();
        let mut estimator = EtaEstimator::new_at(Some(10), start);

        estimator.update_at(1, start + Duration::from_secs(1));

        assert_duration_close(estimator.eta(), Duration::from_secs(9));
        assert!((estimator.percentage() - 10.0).abs() < 0.0001);
    }

    #[test]
    fn eta_uses_exponential_moving_average() {
        let start = Instant::now();
        let mut estimator = EtaEstimator::new_at(Some(1_000), start);

        estimator.update_at(100, start + Duration::from_secs(10));
        estimator.update_at(300, start + Duration::from_secs(20));

        let rate = estimator
            .smoothed_rate()
            .expect("smoothed rate should exist");

        assert!((rate - 13.0).abs() < 0.0001);
    }

    #[test]
    fn percentage_handles_zero_total() {
        let estimator = EtaEstimator::new(Some(0));

        assert_eq!(estimator.percentage(), 100.0);
    }

    #[test]
    fn formatted_eta_uses_hour_format_for_long_duration() {
        let start = Instant::now();
        let mut estimator = EtaEstimator::new_at(Some(7_200), start);

        estimator.update_at(1, start + Duration::from_secs(1));

        assert_eq!(estimator.formatted_eta(), "ETA 1:59:59");
    }
}
