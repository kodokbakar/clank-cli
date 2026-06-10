pub mod histogram;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::config::OutputFormat;
use crate::ui::{
    LiveStats, count_error_color, count_warning_color, error_rate_color, latency_color,
    maybe_color, success_rate_color, throughput_color,
};

pub use histogram::HdrHistogram;

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
    pub http_4xx: u64,
    pub http_5xx: u64,
    pub http_other: u64,
    pub other: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Percentiles {
    pub p50: Duration,
    pub p95: Duration,
    pub p99: Duration,
    pub p999: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Throughput {
    pub requests_per_second: f64,
    pub successful_per_second: f64,
    pub failed_per_second: f64,
    pub total_requests: u64,
    pub duration: Duration,
}

impl Throughput {
    pub fn calculate(
        total_requests: u64,
        successful_requests: u64,
        total_errors: u64,
        duration: Duration,
    ) -> Self {
        let duration_secs = duration.as_secs_f64();

        if duration_secs == 0.0 {
            return Self {
                requests_per_second: 0.0,
                successful_per_second: 0.0,
                failed_per_second: 0.0,
                total_requests,
                duration,
            };
        }

        Self {
            requests_per_second: total_requests as f64 / duration_secs,
            successful_per_second: successful_requests as f64 / duration_secs,
            failed_per_second: total_errors as f64 / duration_secs,
            total_requests,
            duration,
        }
    }
}

impl Default for Throughput {
    fn default() -> Self {
        Self {
            requests_per_second: 0.0,
            successful_per_second: 0.0,
            failed_per_second: 0.0,
            total_requests: 0,
            duration: Duration::ZERO,
        }
    }
}

impl ErrorCounts {
    pub fn total(&self) -> u64 {
        self.timeout
            + self.connection
            + self.http_4xx
            + self.http_5xx
            + self.http_other
            + self.other
    }

    pub fn http_total(&self) -> u64 {
        self.http_4xx + self.http_5xx + self.http_other
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
    histogram: HdrHistogram,
    errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct StatsSnapshot {
    pub duration: Duration,
    pub total_requests: u64,
    pub successful_requests: u64,
    pub total_errors: u64,
    pub error_counts: ErrorCounts,
    pub status_codes: BTreeMap<u16, u64>,
    pub histogram: HdrHistogram,
    pub percentiles: Percentiles,
    pub throughput: Throughput,
    pub latencies: Vec<Duration>,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SummaryJson {
    total_requests: u64,
    successful: u64,
    errors: u64,
    error_rate: f64,
    latency: LatencyJson,
    throughput_rps: f64,
    duration_secs: f64,
    error_breakdown: ErrorBreakdownJson,
}

#[derive(Debug, Serialize)]
struct LatencyJson {
    avg_ms: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    p999_ms: f64,
}

#[derive(Debug, Serialize)]
struct ErrorBreakdownJson {
    timeout: u64,
    connection: u64,
    http_4xx: u64,
    http_5xx: u64,
    http_other: u64,
    other: u64,
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
            histogram: HdrHistogram::new(),
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
        self.histogram.record(latency);

        let count = self.status_codes.entry(status_code).or_insert(0);
        *count += 1;

        if status_code >= 400 {
            self.total_errors += 1;

            match status_code {
                400..=499 => self.error_counts.http_4xx += 1,
                500..=599 => self.error_counts.http_5xx += 1,
                _ => self.error_counts.http_other += 1,
            }
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
            ErrorCategory::Http => self.error_counts.http_other += 1,
            ErrorCategory::Other => self.error_counts.other += 1,
        }

        if self.errors.len() < MAX_STORED_ERRORS {
            self.errors.push(error.to_string());
        }
    }

    pub fn snapshot(&self) -> StatsSnapshot {
        let duration = self.started_at.elapsed();
        let histogram = self.histogram.clone();

        let percentiles = Percentiles {
            p50: histogram.p50(),
            p95: histogram.p95(),
            p99: histogram.p99(),
            p999: histogram.p999(),
        };

        let throughput = Throughput::calculate(
            self.total_requests,
            self.successful_requests,
            self.total_errors,
            duration,
        );

        StatsSnapshot {
            duration,
            total_requests: self.total_requests,
            successful_requests: self.successful_requests,
            total_errors: self.total_errors,
            error_counts: self.error_counts.clone(),
            status_codes: self.status_codes.clone(),
            latencies: histogram.to_durations(),
            histogram,
            percentiles,
            throughput,
            errors: self.errors.clone(),
        }
    }

    pub fn live_snapshot(&self) -> LiveStats {
        LiveStats {
            elapsed: self.started_at.elapsed(),
            total_requests: self.total_requests,
            successful: self.successful_requests,
            errors: self.total_errors,
            current_rps: self.current_rps(),
            avg_latency_ms: self.avg_latency_ms(),
            min_latency_ms: self.min_latency_ms(),
            max_latency_ms: self.max_latency_ms(),
        }
    }

    pub fn current_rps(&self) -> f64 {
        let elapsed_secs = self.started_at.elapsed().as_secs_f64();

        if elapsed_secs > 0.0 {
            self.total_requests as f64 / elapsed_secs
        } else {
            0.0
        }
    }

    fn avg_latency_ms(&self) -> f64 {
        self.histogram
            .mean()
            .map(|duration| duration.as_secs_f64() * 1000.0)
            .unwrap_or(0.0)
    }

    fn min_latency_ms(&self) -> f64 {
        self.histogram
            .min()
            .map(|duration| duration.as_secs_f64() * 1000.0)
            .unwrap_or(0.0)
    }

    fn max_latency_ms(&self) -> f64 {
        self.histogram
            .max()
            .map(|duration| duration.as_secs_f64() * 1000.0)
            .unwrap_or(0.0)
    }
}

pub fn format_summary(snapshot: &StatsSnapshot, output_format: OutputFormat) -> String {
    format_summary_with_color_and_format(snapshot, output_format, false)
}

pub fn format_summary_text(snapshot: &StatsSnapshot) -> String {
    format_summary_text_with_color(snapshot, false)
}

pub fn format_summary_with_color(snapshot: &StatsSnapshot, color_enabled: bool) -> String {
    format_summary_text_with_color(snapshot, color_enabled)
}

pub fn format_summary_with_color_and_format(
    snapshot: &StatsSnapshot,
    output_format: OutputFormat,
    color_enabled: bool,
) -> String {
    match output_format {
        OutputFormat::Text => format_summary_text_with_color(snapshot, color_enabled),
        OutputFormat::Json => format_summary_json(snapshot),
        OutputFormat::Csv => format_summary_csv(snapshot),
    }
}

fn format_summary_text_with_color(snapshot: &StatsSnapshot, color_enabled: bool) -> String {
    let total_requests = snapshot.total_requests;
    let successful = snapshot.successful_requests;
    let errors = snapshot.total_errors;

    let successful_percentage = percentage(successful, total_requests);
    let error_percentage = percentage(errors, total_requests);
    let avg_latency_ms = average_latency_ms(snapshot);
    let p50_ms = duration_to_ms(snapshot.percentiles.p50);
    let p95_ms = duration_to_ms(snapshot.percentiles.p95);
    let p99_ms = duration_to_ms(snapshot.percentiles.p99);
    let p999_ms = duration_to_ms(snapshot.percentiles.p999);
    let duration_secs = snapshot.duration.as_secs_f64();
    let throughput = snapshot.throughput.requests_per_second;

    let total_requests_line = format_number(total_requests);

    let successful_line = maybe_color(
        &format!(
            "{} ({:.1}%)",
            format_number(successful),
            successful_percentage
        ),
        success_rate_color(successful_percentage),
        color_enabled,
    );

    let errors_line = maybe_color(
        &format!("{} ({:.1}%)", format_number(errors), error_percentage),
        error_rate_color(error_percentage),
        color_enabled,
    );

    let timeout_count = maybe_color(
        &format_number(snapshot.error_counts.timeout),
        count_error_color(snapshot.error_counts.timeout),
        color_enabled,
    );

    let connection_count = maybe_color(
        &format_number(snapshot.error_counts.connection),
        count_error_color(snapshot.error_counts.connection),
        color_enabled,
    );

    let http_4xx_count = maybe_color(
        &format_number(snapshot.error_counts.http_4xx),
        count_warning_color(snapshot.error_counts.http_4xx),
        color_enabled,
    );

    let http_5xx_count = maybe_color(
        &format_number(snapshot.error_counts.http_5xx),
        count_error_color(snapshot.error_counts.http_5xx),
        color_enabled,
    );

    let http_other_count = maybe_color(
        &format_number(snapshot.error_counts.http_other),
        count_warning_color(snapshot.error_counts.http_other),
        color_enabled,
    );

    let other_count = maybe_color(
        &format_number(snapshot.error_counts.other),
        count_error_color(snapshot.error_counts.other),
        color_enabled,
    );

    let p99_line = maybe_color(
        &format!("{:.1}ms", p99_ms),
        latency_color(p99_ms),
        color_enabled,
    );

    let throughput_line = maybe_color(
        &format!("{:.1} req/s", throughput),
        throughput_color(throughput),
        color_enabled,
    );

    format!(
        "\
Results:
────────────────────────────────
Total Requests:    {total_requests_line}
Successful:        {successful_line}
Errors:            {errors_line}
  Timeout:         {timeout_count}
  Connection:      {connection_count}
  HTTP 4xx:        {http_4xx_count}
  HTTP 5xx:        {http_5xx_count}
  HTTP Other:      {http_other_count}
  Other:           {other_count}
────────────────────────────────
Latency (avg):     {avg_latency_ms:.1}ms
Latency (p50):     {p50_ms:.1}ms
Latency (p95):     {p95_ms:.1}ms
Latency (p99):     {p99_line}
Latency (p999):    {p999_ms:.1}ms
Throughput:        {throughput_line}
Duration:          {duration_secs:.2}s
────────────────────────────────"
    )
}

pub fn format_summary_json(snapshot: &StatsSnapshot) -> String {
    let summary = SummaryJson {
        total_requests: snapshot.total_requests,
        successful: snapshot.successful_requests,
        errors: snapshot.total_errors,
        error_rate: percentage(snapshot.total_errors, snapshot.total_requests),
        latency: LatencyJson {
            avg_ms: average_latency_ms(snapshot),
            p50_ms: duration_to_ms(snapshot.percentiles.p50),
            p95_ms: duration_to_ms(snapshot.percentiles.p95),
            p99_ms: duration_to_ms(snapshot.percentiles.p99),
            p999_ms: duration_to_ms(snapshot.percentiles.p999),
        },
        throughput_rps: snapshot.throughput.requests_per_second,
        duration_secs: snapshot.duration.as_secs_f64(),
        error_breakdown: ErrorBreakdownJson {
            timeout: snapshot.error_counts.timeout,
            connection: snapshot.error_counts.connection,
            http_4xx: snapshot.error_counts.http_4xx,
            http_5xx: snapshot.error_counts.http_5xx,
            http_other: snapshot.error_counts.http_other,
            other: snapshot.error_counts.other,
        },
    };

    serde_json::to_string_pretty(&summary).expect("summary JSON serialization should not fail")
}

pub fn format_summary_csv(snapshot: &StatsSnapshot) -> String {
    let header = "total_requests,successful,errors,error_rate,avg_ms,p50_ms,p95_ms,p99_ms,p999_ms,throughput_rps,duration_secs,timeout,connection,http_4xx,http_5xx,http_other,other";

    let row = format!(
        "{},{},{},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.2},{},{},{},{},{},{}",
        snapshot.total_requests,
        snapshot.successful_requests,
        snapshot.total_errors,
        percentage(snapshot.total_errors, snapshot.total_requests),
        average_latency_ms(snapshot),
        duration_to_ms(snapshot.percentiles.p50),
        duration_to_ms(snapshot.percentiles.p95),
        duration_to_ms(snapshot.percentiles.p99),
        duration_to_ms(snapshot.percentiles.p999),
        snapshot.throughput.requests_per_second,
        snapshot.duration.as_secs_f64(),
        snapshot.error_counts.timeout,
        snapshot.error_counts.connection,
        snapshot.error_counts.http_4xx,
        snapshot.error_counts.http_5xx,
        snapshot.error_counts.http_other,
        snapshot.error_counts.other,
    );

    format!("{header}\n{row}")
}

fn percentage(value: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        value as f64 / total as f64 * 100.0
    }
}

fn average_latency_ms(snapshot: &StatsSnapshot) -> f64 {
    snapshot
        .histogram
        .mean()
        .map(|duration| duration.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

fn duration_to_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
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

    fn assert_float_close(actual: f64, expected: f64) {
        let diff = (actual - expected).abs();

        assert!(
            diff < 0.0001,
            "actual: {actual}, expected: {expected}, diff: {diff}"
        );
    }

    fn summary_snapshot() -> StatsSnapshot {
        let mut stats = StatsCollector::new();

        for _ in 0..80 {
            stats.record(Duration::from_millis(40), 200);
        }

        for _ in 0..10 {
            stats.record(Duration::from_millis(80), 404);
        }

        for _ in 0..10 {
            stats.record_error(ErrorCategory::Timeout, "request timed out");
        }

        stats.snapshot()
    }

    #[test]
    fn format_summary_dispatches_text_output() {
        let snapshot = summary_snapshot();
        let output = format_summary(&snapshot, OutputFormat::Text);

        assert!(output.contains("Results:"));
        assert!(output.contains("Total Requests:"));
        assert!(output.contains("Throughput:"));
    }

    #[test]
    fn format_summary_json_outputs_valid_json() -> serde_json::Result<()> {
        let snapshot = summary_snapshot();
        let output = format_summary_json(&snapshot);
        let value: serde_json::Value = serde_json::from_str(&output)?;

        assert_eq!(value["total_requests"], 100);
        assert_eq!(value["successful"], 80);
        assert_eq!(value["errors"], 20);
        assert_eq!(value["error_breakdown"]["timeout"], 10);
        assert!(value["latency"]["p99_ms"].is_number());
        assert!(value["throughput_rps"].is_number());
        assert!(value["duration_secs"].is_number());

        Ok(())
    }

    #[test]
    fn format_summary_dispatches_json_output() -> serde_json::Result<()> {
        let snapshot = summary_snapshot();
        let output = format_summary(&snapshot, OutputFormat::Json);
        let value: serde_json::Value = serde_json::from_str(&output)?;

        assert_eq!(value["total_requests"], 100);

        Ok(())
    }

    #[test]
    fn format_summary_csv_outputs_header_and_one_row() {
        let snapshot = summary_snapshot();
        let output = format_summary_csv(&snapshot);
        let lines = output.lines().collect::<Vec<_>>();

        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0],
            "total_requests,successful,errors,error_rate,avg_ms,p50_ms,p95_ms,p99_ms,p999_ms,throughput_rps,duration_secs,timeout,connection,http_4xx,http_5xx,http_other,other"
        );
        assert!(lines[1].starts_with("100,80,20,20.0,"));
    }

    #[test]
    fn format_summary_dispatches_csv_output() {
        let snapshot = summary_snapshot();
        let output = format_summary(&snapshot, OutputFormat::Csv);
        let lines = output.lines().collect::<Vec<_>>();

        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn throughput_calculates_normal_rates() {
        let throughput = Throughput::calculate(100, 80, 20, Duration::from_secs(10));

        assert_float_close(throughput.requests_per_second, 10.0);
        assert_float_close(throughput.successful_per_second, 8.0);
        assert_float_close(throughput.failed_per_second, 2.0);
        assert_eq!(throughput.total_requests, 100);
        assert_eq!(throughput.duration, Duration::from_secs(10));
    }

    #[test]
    fn throughput_returns_zero_for_zero_duration() {
        let throughput = Throughput::calculate(100, 80, 20, Duration::ZERO);

        assert_float_close(throughput.requests_per_second, 0.0);
        assert_float_close(throughput.successful_per_second, 0.0);
        assert_float_close(throughput.failed_per_second, 0.0);
        assert_eq!(throughput.total_requests, 100);
        assert_eq!(throughput.duration, Duration::ZERO);
    }

    #[test]
    fn throughput_returns_zero_for_zero_requests() {
        let throughput = Throughput::calculate(0, 0, 0, Duration::from_secs(10));

        assert_float_close(throughput.requests_per_second, 0.0);
        assert_float_close(throughput.successful_per_second, 0.0);
        assert_float_close(throughput.failed_per_second, 0.0);
        assert_eq!(throughput.total_requests, 0);
        assert_eq!(throughput.duration, Duration::from_secs(10));
    }

    #[test]
    fn throughput_handles_one_request_in_short_duration() {
        let throughput = Throughput::calculate(1, 1, 0, Duration::from_millis(1));

        assert_float_close(throughput.requests_per_second, 1000.0);
        assert_float_close(throughput.successful_per_second, 1000.0);
        assert_float_close(throughput.failed_per_second, 0.0);
        assert_eq!(throughput.total_requests, 1);
    }

    #[test]
    fn snapshot_includes_throughput() {
        let mut stats = StatsCollector::new();

        stats.record(Duration::from_millis(10), 200);
        stats.record(Duration::from_millis(20), 503);

        let snapshot = stats.snapshot();

        assert_eq!(snapshot.throughput.total_requests, 2);
        assert!(snapshot.throughput.requests_per_second > 0.0);
        assert!(snapshot.throughput.successful_per_second > 0.0);
        assert!(snapshot.throughput.failed_per_second > 0.0);
    }

    #[test]
    fn format_summary_uses_throughput_struct() {
        let snapshot = StatsSnapshot {
            duration: Duration::from_secs(10),
            total_requests: 100,
            successful_requests: 80,
            total_errors: 20,
            error_counts: ErrorCounts {
                timeout: 0,
                connection: 0,
                http_4xx: 0,
                http_5xx: 20,
                http_other: 0,
                other: 0,
            },
            status_codes: BTreeMap::new(),
            histogram: HdrHistogram::new(),
            percentiles: Percentiles::default(),
            throughput: Throughput::calculate(100, 80, 20, Duration::from_secs(10)),
            latencies: Vec::new(),
            errors: Vec::new(),
        };

        let output = format_summary_text(&snapshot);

        assert!(output.contains("Throughput:        10.0 req/s"));
    }

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
        assert_eq!(snapshot.histogram.len(), 3);
        assert_eq!(snapshot.latencies.len(), 3);
    }

    #[test]
    fn record_counts_http_5xx_status_error() {
        let mut stats = StatsCollector::new();

        stats.record(Duration::from_millis(25), 503);

        let snapshot = stats.snapshot();

        assert_eq!(snapshot.total_requests, 1);
        assert_eq!(snapshot.successful_requests, 0);
        assert_eq!(snapshot.total_errors, 1);
        assert_eq!(snapshot.error_counts.http_4xx, 0);
        assert_eq!(snapshot.error_counts.http_5xx, 1);
        assert_eq!(snapshot.error_counts.http_other, 0);
        assert_eq!(snapshot.status_codes.get(&503), Some(&1));
        assert_eq!(snapshot.histogram.len(), 1);
    }

    #[test]
    fn record_counts_mixed_error_categories() {
        let mut stats = StatsCollector::new();

        stats.record(Duration::from_millis(10), 200);
        stats.record(Duration::from_millis(20), 404);
        stats.record(Duration::from_millis(30), 503);
        stats.record_error(ErrorCategory::Timeout, "request timed out");
        stats.record_error(ErrorCategory::Connection, "connection refused");
        stats.record_error(ErrorCategory::Other, "unsupported method");

        let snapshot = stats.snapshot();

        assert_eq!(snapshot.total_requests, 6);
        assert_eq!(snapshot.successful_requests, 1);
        assert_eq!(snapshot.total_errors, 5);
        assert_eq!(snapshot.error_counts.timeout, 1);
        assert_eq!(snapshot.error_counts.connection, 1);
        assert_eq!(snapshot.error_counts.http_4xx, 1);
        assert_eq!(snapshot.error_counts.http_5xx, 1);
        assert_eq!(snapshot.error_counts.http_other, 0);
        assert_eq!(snapshot.error_counts.other, 1);
        assert_eq!(snapshot.error_counts.total(), 5);
        assert_eq!(snapshot.error_counts.http_total(), 2);
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
        assert_eq!(snapshot.histogram.len(), 0);
        assert_eq!(snapshot.latencies.len(), 0);
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
        assert_eq!(snapshot.histogram.len(), 0);
    }

    #[test]
    fn format_summary_handles_zero_requests() {
        let stats = StatsCollector::new();
        let snapshot = stats.snapshot();
        let output = format_summary_text(&snapshot);

        assert!(output.contains("Total Requests:    0"));
        assert!(output.contains("Successful:        0 (0.0%)"));
        assert!(output.contains("Errors:            0 (0.0%)"));
        assert!(output.contains("Latency (avg):     0.0ms"));
        assert!(output.contains("Latency (p50):     0.0ms"));
        assert!(output.contains("Latency (p95):     0.0ms"));
        assert!(output.contains("Latency (p99):     0.0ms"));
        assert!(output.contains("Latency (p999):    0.0ms"));
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

        let output = format_summary_text(&stats.snapshot());

        assert!(output.contains("Total Requests:    1,234"));
        assert!(output.contains("Successful:        1,200 (97.2%)"));
        assert!(output.contains("Errors:            34 (2.8%)"));
        assert!(output.contains("HTTP 4xx:        0"));
        assert!(output.contains("HTTP 5xx:        34"));
        assert!(output.contains("HTTP Other:      0"));
        assert!(output.contains("Latency (avg):"));
        assert!(output.contains("Latency (p50):"));
        assert!(output.contains("Latency (p95):"));
        assert!(output.contains("Latency (p99):"));
        assert!(output.contains("Latency (p999):"));
    }

    #[test]
    fn histogram_records_more_than_one_thousand_latency_samples() {
        let mut stats = StatsCollector::new();

        for micros in 1..=1000 {
            stats.record(Duration::from_micros(micros), 200);
        }

        let snapshot = stats.snapshot();

        assert_eq!(snapshot.total_requests, 1000);
        assert_eq!(snapshot.successful_requests, 1000);
        assert_eq!(snapshot.histogram.len(), 1000);
        assert_eq!(snapshot.latencies.len(), 1000);

        let mean_us = snapshot.histogram.mean().unwrap().as_secs_f64() * 1_000_000.0;

        assert!((mean_us - 500.5).abs() < 2.0);
    }

    #[test]
    fn snapshot_includes_latency_percentiles() {
        let mut stats = StatsCollector::new();

        for value in 1..=100 {
            stats.record(Duration::from_millis(value), 200);
        }

        let snapshot = stats.snapshot();

        assert!(snapshot.percentiles.p50 >= Duration::from_millis(49));
        assert!(snapshot.percentiles.p50 <= Duration::from_millis(51));

        assert!(snapshot.percentiles.p95 >= Duration::from_millis(94));
        assert!(snapshot.percentiles.p95 <= Duration::from_millis(96));

        assert!(snapshot.percentiles.p99 >= Duration::from_millis(98));
        assert!(snapshot.percentiles.p99 <= Duration::from_millis(100));

        assert!(snapshot.percentiles.p999 >= Duration::from_millis(99));
        assert!(snapshot.percentiles.p999 <= Duration::from_millis(101));
    }

    #[test]
    fn record_counts_http_4xx_status_error() {
        let mut stats = StatsCollector::new();

        stats.record(Duration::from_millis(25), 404);

        let snapshot = stats.snapshot();

        assert_eq!(snapshot.total_requests, 1);
        assert_eq!(snapshot.successful_requests, 0);
        assert_eq!(snapshot.total_errors, 1);
        assert_eq!(snapshot.error_counts.http_4xx, 1);
        assert_eq!(snapshot.error_counts.http_5xx, 0);
        assert_eq!(snapshot.error_counts.http_other, 0);
        assert_eq!(snapshot.status_codes.get(&404), Some(&1));
    }

    #[test]
    fn record_error_http_category_counts_as_http_other() {
        let mut stats = StatsCollector::new();

        stats.record_error(ErrorCategory::Http, "HTTP error without status code");

        let snapshot = stats.snapshot();

        assert_eq!(snapshot.total_requests, 1);
        assert_eq!(snapshot.successful_requests, 0);
        assert_eq!(snapshot.total_errors, 1);
        assert_eq!(snapshot.error_counts.http_4xx, 0);
        assert_eq!(snapshot.error_counts.http_5xx, 0);
        assert_eq!(snapshot.error_counts.http_other, 1);
        assert_eq!(snapshot.error_counts.other, 0);
    }

    #[test]
    fn live_snapshot_returns_current_stats_without_cloning_snapshot_histogram() {
        let mut stats = StatsCollector::new();

        stats.record(Duration::from_millis(10), 200);
        stats.record(Duration::from_millis(20), 200);
        stats.record(Duration::from_millis(30), 503);
        stats.record_error(ErrorCategory::Connection, "connection refused");

        let live = stats.live_snapshot();

        assert_eq!(live.total_requests, 4);
        assert_eq!(live.successful, 2);
        assert_eq!(live.errors, 2);
        assert!(live.current_rps > 0.0);
        assert!(live.avg_latency_ms > 0.0);
        assert!(live.min_latency_ms > 0.0);
        assert!(live.max_latency_ms >= live.min_latency_ms);
    }

    #[test]
    fn current_rps_returns_zero_when_no_requests_recorded() {
        let stats = StatsCollector::new();

        assert_eq!(stats.current_rps(), 0.0);
    }

    #[test]
    fn live_snapshot_handles_empty_latency_histogram() {
        let mut stats = StatsCollector::new();

        stats.record_error(ErrorCategory::Timeout, "request timed out");

        let live = stats.live_snapshot();

        assert_eq!(live.total_requests, 1);
        assert_eq!(live.successful, 0);
        assert_eq!(live.errors, 1);
        assert_eq!(live.avg_latency_ms, 0.0);
        assert_eq!(live.min_latency_ms, 0.0);
        assert_eq!(live.max_latency_ms, 0.0);
    }

    #[test]
    fn format_summary_with_color_disabled_matches_plain_summary() {
        let mut stats = StatsCollector::new();

        stats.record(Duration::from_millis(45), 200);
        stats.record(Duration::from_millis(50), 503);

        let snapshot = stats.snapshot();

        assert_eq!(
            format_summary_text(&snapshot),
            format_summary_with_color(&snapshot, false)
        );
    }

    #[test]
    fn format_summary_with_color_keeps_plain_content_visible() {
        let mut stats = StatsCollector::new();

        for _ in 0..100 {
            stats.record(Duration::from_millis(45), 200);
        }

        let output = format_summary_with_color(&stats.snapshot(), true);

        assert!(output.contains("Successful:"));
        assert!(output.contains("Errors:"));
        assert!(output.contains("Latency (p99):"));
        assert!(output.contains("Throughput:"));
    }
}
