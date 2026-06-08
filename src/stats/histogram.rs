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
}
