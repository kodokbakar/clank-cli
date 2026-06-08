use std::time::Duration;

use hdrhistogram::Histogram;

#[derive(Debug, Clone)]
pub struct HdrHistogram {
    inner: Histogram<u64>,
}

impl HdrHistogram {
    pub fn new() -> Self {
        let inner = Histogram::<u64>::new(3).expect("valid histogram precision");

        Self { inner }
    }

    pub fn record(&mut self, duration: Duration) {
        let micros = duration_to_micros(duration);

        self.inner
            .record(micros)
            .expect("auto-resizing histogram should record latency");
    }

    pub fn record_many(&mut self, latencies: &[Duration]) {
        for latency in latencies {
            self.record(*latency);
        }
    }

    pub fn min(&self) -> Option<Duration> {
        if self.is_empty() {
            return None;
        }

        Some(Duration::from_micros(self.inner.min()))
    }

    pub fn max(&self) -> Option<Duration> {
        if self.is_empty() {
            return None;
        }

        Some(Duration::from_micros(self.inner.max()))
    }

    pub fn mean(&self) -> Option<Duration> {
        if self.is_empty() {
            return None;
        }

        Some(Duration::from_secs_f64(self.inner.mean() / 1_000_000.0))
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn len(&self) -> u64 {
        self.inner.len()
    }

    pub fn to_durations(&self) -> Vec<Duration> {
        let mut latencies = Vec::with_capacity(self.len() as usize);

        for value in self.inner.iter_recorded() {
            let duration = Duration::from_micros(value.value_iterated_to());

            for _ in 0..value.count_since_last_iteration() {
                latencies.push(duration);
            }
        }

        latencies
    }

    pub fn percentile(&self, p: f64) -> Duration {
        if self.is_empty() {
            return Duration::ZERO;
        }

        let quantile = if p.is_nan() { 0.0 } else { p.clamp(0.0, 1.0) };

        Duration::from_micros(self.inner.value_at_quantile(quantile))
    }

    pub fn p50(&self) -> Duration {
        self.percentile(0.50)
    }

    pub fn p95(&self) -> Duration {
        self.percentile(0.95)
    }

    pub fn p99(&self) -> Duration {
        self.percentile(0.99)
    }

    pub fn p999(&self) -> Duration {
        self.percentile(0.999)
    }
}

impl Default for HdrHistogram {
    fn default() -> Self {
        Self::new()
    }
}

fn duration_to_micros(duration: Duration) -> u64 {
    let micros = duration.as_micros();

    if micros == 0 {
        1
    } else if micros > u64::MAX as u128 {
        u64::MAX
    } else {
        micros as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_duration_close(actual: Option<Duration>, expected: Duration) {
        let actual = actual.expect("duration should exist");
        let diff = actual.abs_diff(expected);

        assert!(
            diff <= Duration::from_micros(100),
            "actual: {:?}, expected: {:?}, diff: {:?}",
            actual,
            expected,
            diff
        );
    }

    #[test]
    fn record_stores_latency_sample() {
        let mut histogram = HdrHistogram::new();

        histogram.record(Duration::from_millis(10));

        assert_eq!(histogram.len(), 1);
        assert!(!histogram.is_empty());
        assert_duration_close(histogram.min(), Duration::from_millis(10));
        assert_duration_close(histogram.max(), Duration::from_millis(10));
    }

    #[test]
    fn record_many_stores_multiple_samples() {
        let mut histogram = HdrHistogram::new();

        histogram.record_many(&[
            Duration::from_millis(10),
            Duration::from_millis(20),
            Duration::from_millis(30),
        ]);

        assert_eq!(histogram.len(), 3);
        assert_duration_close(histogram.min(), Duration::from_millis(10));
        assert_duration_close(histogram.max(), Duration::from_millis(30));
    }

    #[test]
    fn empty_histogram_returns_no_stats() {
        let histogram = HdrHistogram::new();

        assert!(histogram.is_empty());
        assert_eq!(histogram.len(), 0);
        assert_eq!(histogram.min(), None);
        assert_eq!(histogram.max(), None);
        assert_eq!(histogram.mean(), None);
    }

    #[test]
    fn mean_returns_average_latency() {
        let mut histogram = HdrHistogram::new();

        histogram.record(Duration::from_millis(10));
        histogram.record(Duration::from_millis(20));
        histogram.record(Duration::from_millis(30));

        let mean = histogram.mean().unwrap();

        assert!(mean >= Duration::from_millis(19));
        assert!(mean <= Duration::from_millis(21));
    }

    #[test]
    fn histogram_handles_more_than_one_thousand_samples() {
        let mut histogram = HdrHistogram::new();

        for micros in 1..=1000 {
            histogram.record(Duration::from_micros(micros));
        }

        assert_eq!(histogram.len(), 1000);
        assert_eq!(histogram.min(), Some(Duration::from_micros(1)));
        assert!(histogram.max().unwrap() >= Duration::from_micros(1000));

        let mean_us = histogram.mean().unwrap().as_secs_f64() * 1_000_000.0;

        assert!((mean_us - 500.5).abs() < 2.0);
    }

    #[test]
    fn to_durations_preserves_sample_count_for_snapshot_compatibility() {
        let mut histogram = HdrHistogram::new();

        histogram.record(Duration::from_millis(10));
        histogram.record(Duration::from_millis(20));
        histogram.record(Duration::from_millis(20));

        let latencies = histogram.to_durations();

        assert_eq!(latencies.len(), 3);
    }

    #[test]
    fn percentile_returns_zero_for_empty_histogram() {
        let histogram = HdrHistogram::new();

        assert_eq!(histogram.percentile(0.50), Duration::ZERO);
        assert_eq!(histogram.p50(), Duration::ZERO);
        assert_eq!(histogram.p95(), Duration::ZERO);
        assert_eq!(histogram.p99(), Duration::ZERO);
        assert_eq!(histogram.p999(), Duration::ZERO);
    }

    #[test]
    fn percentile_calculates_common_latency_percentiles() {
        let mut histogram = HdrHistogram::new();

        for value in 1..=100 {
            histogram.record(Duration::from_millis(value));
        }

        assert_duration_close(Some(histogram.p50()), Duration::from_millis(50));
        assert_duration_close(Some(histogram.p95()), Duration::from_millis(95));
        assert_duration_close(Some(histogram.p99()), Duration::from_millis(99));
        assert_duration_close(Some(histogram.p999()), Duration::from_millis(100));
    }

    #[test]
    fn percentile_is_accurate_for_large_dataset() {
        let mut histogram = HdrHistogram::new();

        for value in 1..=10_000 {
            histogram.record(Duration::from_micros(value));
        }

        let p50_us = histogram.p50().as_secs_f64() * 1_000_000.0;
        let p95_us = histogram.p95().as_secs_f64() * 1_000_000.0;
        let p99_us = histogram.p99().as_secs_f64() * 1_000_000.0;
        let p999_us = histogram.p999().as_secs_f64() * 1_000_000.0;

        assert!((p50_us - 5_000.0).abs() < 50.0);
        assert!((p95_us - 9_500.0).abs() < 100.0);
        assert!((p99_us - 9_900.0).abs() < 100.0);
        assert!((p999_us - 9_990.0).abs() < 100.0);
    }

    #[test]
    fn percentile_clamps_invalid_quantile_range() {
        let mut histogram = HdrHistogram::new();

        histogram.record(Duration::from_millis(10));
        histogram.record(Duration::from_millis(20));

        assert_duration_close(Some(histogram.percentile(-1.0)), Duration::from_millis(10));
        assert_duration_close(Some(histogram.percentile(2.0)), Duration::from_millis(20));
    }
}
