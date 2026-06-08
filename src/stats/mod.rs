use std::collections::BTreeMap;
use std::time::Duration;

const MAX_STORED_ERRORS: usize = 100;

#[derive(Debug, Default)]
pub struct StatsCollector {
    total_requests: u64,
    total_errors: u64,
    status_codes: BTreeMap<u16, u64>,
    latencies: Vec<Duration>,
    errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatsSnapshot {
    pub total_requests: u64,
    pub total_errors: u64,
    pub status_codes: BTreeMap<u16, u64>,
    pub latencies: Vec<Duration>,
    pub errors: Vec<String>,
}

impl StatsCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, latency: Duration, status_code: u16) {
        self.total_requests += 1;
        self.latencies.push(latency);

        let count = self.status_codes.entry(status_code).or_insert(0);
        *count += 1;
    }

    pub fn record_error(&mut self, error: impl ToString) {
        self.total_errors += 1;

        if self.errors.len() < MAX_STORED_ERRORS {
            self.errors.push(error.to_string());
        }
    }

    pub fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            total_requests: self.total_requests,
            total_errors: self.total_errors,
            status_codes: self.status_codes.clone(),
            latencies: self.latencies.clone(),
            errors: self.errors.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_increments_total_requests_and_status_code_count() {
        let mut stats = StatsCollector::new();

        stats.record(Duration::from_millis(25), 200);
        stats.record(Duration::from_millis(30), 200);
        stats.record(Duration::from_millis(40), 404);

        let snapshot = stats.snapshot();

        assert_eq!(snapshot.total_requests, 3);
        assert_eq!(snapshot.total_errors, 0);
        assert_eq!(snapshot.status_codes.get(&200), Some(&2));
        assert_eq!(snapshot.status_codes.get(&404), Some(&1));
        assert_eq!(snapshot.latencies.len(), 3);
    }

    #[test]
    fn record_error_increments_total_errors() {
        let mut stats = StatsCollector::new();

        stats.record_error("connection refused");

        let snapshot = stats.snapshot();

        assert_eq!(snapshot.total_requests, 0);
        assert_eq!(snapshot.total_errors, 1);
        assert_eq!(snapshot.errors.len(), 1);
    }

    #[test]
    fn record_error_limits_stored_error_messages() {
        let mut stats = StatsCollector::new();

        for index in 0..150 {
            stats.record_error(format!("error-{index}"));
        }

        let snapshot = stats.snapshot();

        assert_eq!(snapshot.total_errors, 150);
        assert_eq!(snapshot.errors.len(), MAX_STORED_ERRORS);
    }
}
