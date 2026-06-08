use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const MAX_STORED_ERRORS: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    Timeout,
    Connection,
    Http,
    Other,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ErrorCounts {
    pub timeout: u64,
    pub connection: u64,
    pub http: u64,
    pub other: u64,
}

impl ErrorCounts {
    pub fn total(&self) -> u64 {
        self.timeout + self.connection + self.http + self.other
    }
}

#[derive(Debug)]
pub struct StatsCollector {
    started_at: Instant,
    total_requests: u64,
    successful_requests: u64,
    total_errors: u64,
    error_counts: ErrorCounts,
    status_codes: BTreeMap<u16, u64>,
    latencies: Vec<Duration>,
    errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatsSnapshot {
    pub duration: Duration,
    pub total_requests: u64,
    pub successful_requests: u64,
    pub total_errors: u64,
    pub error_counts: ErrorCounts,
    pub status_codes: BTreeMap<u16, u64>,
    pub latencies: Vec<Duration>,
    pub errors: Vec<String>,
}

impl Default for StatsCollector {
    fn default() -> Self {
        Self {
            started_at: Instant::now(),
            total_requests: 0,
            successful_requests: 0,
            total_errors: 0,
            error_counts: ErrorCounts::default(),
            status_codes: BTreeMap::new(),
            latencies: Vec::new(),
            errors: Vec::new(),
        }
    }
}

impl StatsCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset_timer(&mut self) {
        self.started_at = Instant::now();
    }

    pub fn record(&mut self, latency: Duration, status_code: u16) {
        self.total_requests += 1;
        self.latencies.push(latency);

        let count = self.status_codes.entry(status_code).or_insert(0);
        *count += 1;

        if status_code >= 400 {
            self.total_errors += 1;
            self.error_counts.http += 1;
        } else {
            self.successful_requests += 1;
        }
    }

    pub fn record_error(&mut self, category: ErrorCategory, error: impl ToString) {
        self.total_requests += 1;
        self.total_errors += 1;

        match category {
            ErrorCategory::Timeout => self.error_counts.timeout += 1,
            ErrorCategory::Connection => self.error_counts.connection += 1,
            ErrorCategory::Http => self.error_counts.http += 1,
            ErrorCategory::Other => self.error_counts.other += 1,
        }

        if self.errors.len() < MAX_STORED_ERRORS {
            self.errors.push(error.to_string());
        }
    }

    pub fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            duration: self.started_at.elapsed(),
            total_requests: self.total_requests,
            successful_requests: self.successful_requests,
            total_errors: self.total_errors,
            error_counts: self.error_counts.clone(),
            status_codes: self.status_codes.clone(),
            latencies: self.latencies.clone(),
            errors: self.errors.clone(),
        }
    }
}

pub fn format_summary(snapshot: &StatsSnapshot) -> String {
    let total_requests = snapshot.total_requests;
    let successful = snapshot.successful_requests;
    let errors = snapshot.total_errors;

    let successful_percentage = percentage(successful, total_requests);
    let error_percentage = percentage(errors, total_requests);
    let avg_latency_ms = average_latency_ms(&snapshot.latencies);
    let duration_secs = snapshot.duration.as_secs_f64();
    let throughput = if duration_secs > 0.0 {
        total_requests as f64 / duration_secs
    } else {
        0.0
    };

    format!(
        "\
Results:
────────────────────────────────
Total Requests:    {}
Successful:        {} ({:.1}%)
Errors:            {} ({:.1}%)
  Timeout:         {}
  Connection:      {}
  HTTP Error:      {}
  Other:           {}
────────────────────────────────
Latency (avg):     {:.1}ms
Throughput:        {:.1} req/s
Duration:          {:.2}s
────────────────────────────────",
        format_number(total_requests),
        format_number(successful),
        successful_percentage,
        format_number(errors),
        error_percentage,
        format_number(snapshot.error_counts.timeout),
        format_number(snapshot.error_counts.connection),
        format_number(snapshot.error_counts.http),
        format_number(snapshot.error_counts.other),
        avg_latency_ms,
        throughput,
        duration_secs
    )
}

fn percentage(value: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        value as f64 / total as f64 * 100.0
    }
}

fn average_latency_ms(latencies: &[Duration]) -> f64 {
    if latencies.is_empty() {
        return 0.0;
    }

    let total_ms: f64 = latencies
        .iter()
        .map(|duration| duration.as_secs_f64() * 1000.0)
        .sum();

    total_ms / latencies.len() as f64
}

fn format_number(value: u64) -> String {
    let digits = value.to_string();
    let mut result = String::new();

    for (index, char) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            result.push(',');
        }

        result.push(char);
    }

    result.chars().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_increments_successful_requests_and_status_code_count() {
        let mut stats = StatsCollector::new();

        stats.record(Duration::from_millis(25), 200);
        stats.record(Duration::from_millis(30), 200);
        stats.record(Duration::from_millis(40), 302);

        let snapshot = stats.snapshot();

        assert_eq!(snapshot.total_requests, 3);
        assert_eq!(snapshot.successful_requests, 3);
        assert_eq!(snapshot.total_errors, 0);
        assert_eq!(snapshot.status_codes.get(&200), Some(&2));
        assert_eq!(snapshot.status_codes.get(&302), Some(&1));
        assert_eq!(snapshot.latencies.len(), 3);
    }

    #[test]
    fn record_counts_http_status_error() {
        let mut stats = StatsCollector::new();

        stats.record(Duration::from_millis(25), 503);

        let snapshot = stats.snapshot();

        assert_eq!(snapshot.total_requests, 1);
        assert_eq!(snapshot.successful_requests, 0);
        assert_eq!(snapshot.total_errors, 1);
        assert_eq!(snapshot.error_counts.http, 1);
        assert_eq!(snapshot.status_codes.get(&503), Some(&1));
    }

    #[test]
    fn record_error_increments_total_errors_by_category() {
        let mut stats = StatsCollector::new();

        stats.record_error(ErrorCategory::Connection, "connection refused");

        let snapshot = stats.snapshot();

        assert_eq!(snapshot.total_requests, 1);
        assert_eq!(snapshot.successful_requests, 0);
        assert_eq!(snapshot.total_errors, 1);
        assert_eq!(snapshot.error_counts.connection, 1);
        assert_eq!(snapshot.errors.len(), 1);
    }

    #[test]
    fn record_error_limits_stored_error_messages() {
        let mut stats = StatsCollector::new();

        for index in 0..150 {
            stats.record_error(ErrorCategory::Other, format!("error-{index}"));
        }

        let snapshot = stats.snapshot();

        assert_eq!(snapshot.total_requests, 150);
        assert_eq!(snapshot.total_errors, 150);
        assert_eq!(snapshot.errors.len(), MAX_STORED_ERRORS);
    }

    #[test]
    fn format_summary_handles_zero_requests() {
        let stats = StatsCollector::new();
        let snapshot = stats.snapshot();
        let output = format_summary(&snapshot);

        assert!(output.contains("Total Requests:    0"));
        assert!(output.contains("Successful:        0 (0.0%)"));
        assert!(output.contains("Errors:            0 (0.0%)"));
        assert!(output.contains("Latency (avg):     0.0ms"));
    }

    #[test]
    fn format_summary_formats_numbers_percentages_and_latency() {
        let mut stats = StatsCollector::new();

        for _ in 0..1200 {
            stats.record(Duration::from_millis(45), 200);
        }

        for _ in 0..34 {
            stats.record(Duration::from_millis(50), 503);
        }

        let output = format_summary(&stats.snapshot());

        assert!(output.contains("Total Requests:    1,234"));
        assert!(output.contains("Successful:        1,200 (97.2%)"));
        assert!(output.contains("Errors:            34 (2.8%)"));
        assert!(output.contains("HTTP Error:      34"));
    }
}
