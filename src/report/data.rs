use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::config::RateLimitConfig;
use crate::engine::ValidationConfig;
use crate::stats::{ErrorCounts, StatsSnapshot};

#[derive(Debug, Clone)]
pub struct HtmlReportContext<'a> {
    pub snapshot: &'a StatsSnapshot,
    pub target_url: &'a str,
    pub command: &'a str,
    pub rate_limit: Option<&'a RateLimitConfig>,
    pub generated_at: SystemTime,
    pub app_version: &'a str,
    pub concurrency: usize,
    pub validation: Option<&'a ValidationConfig>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HtmlReportData {
    pub metadata: ReportMetadata,
    pub summary: ReportSummary,
    pub latency: ReportLatency,
    pub latency_histogram: Vec<LatencyHistogramBucket>,
    pub status_codes: Vec<StatusCodeReport>,
    pub errors: Vec<ErrorReport>,
    pub timestamps_series: Vec<TimelinePoint>,
    pub validation: Option<ValidationReport>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ReportMetadata {
    pub timestamp: u64,
    pub target_url: String,
    pub duration_secs: f64,
    pub concurrency: usize,
    pub version: String,
    pub command: String,
    pub rate_limit: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ReportSummary {
    pub total_requests: u64,
    pub successful: u64,
    pub failed: u64,
    pub validation_errors: usize,
    pub retries: usize,
    pub rps: f64,
    pub avg_latency_ms: f64,
    pub success_rate: f64,
    pub error_rate: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ReportLatency {
    pub min_ms: f64,
    pub p50_ms: f64,
    pub p90_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LatencyHistogramBucket {
    pub bucket_label: String,
    pub count: u64,
    pub percentage: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StatusCodeReport {
    pub code: u16,
    pub count: u64,
    pub percentage: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ErrorReport {
    #[serde(rename = "type")]
    pub error_type: String,
    pub count: u64,
    pub percentage: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TimelinePoint {
    pub elapsed_secs: f64,
    pub rps: f64,
    pub latency_p50: f64,
    pub latency_p99: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ValidationReport {
    pub expected_status: Option<Vec<u16>>,
    pub expected_body: Option<String>,
    pub expected_headers: Vec<String>,
    pub failures: Vec<String>,
}

impl HtmlReportData {
    pub fn from_stats_snapshot(context: &HtmlReportContext<'_>) -> Self {
        let snapshot = context.snapshot;
        let duration_secs = snapshot.duration.as_secs_f64();

        Self {
            metadata: ReportMetadata {
                timestamp: unix_secs(context.generated_at),
                target_url: context.target_url.to_string(),
                duration_secs,
                concurrency: context.concurrency,
                version: context.app_version.to_string(),
                command: context.command.to_string(),
                rate_limit: context
                    .rate_limit
                    .map(RateLimitConfig::as_display_string)
                    .unwrap_or_else(|| "unlimited".to_string()),
            },
            summary: ReportSummary {
                total_requests: snapshot.total_requests,
                successful: snapshot.successful_requests,
                failed: snapshot.total_errors,
                validation_errors: snapshot.validation_errors,
                retries: snapshot.retries,
                rps: snapshot.throughput.requests_per_second,
                avg_latency_ms: average_latency_ms(snapshot),
                success_rate: percentage(snapshot.successful_requests, snapshot.total_requests),
                error_rate: percentage(snapshot.total_errors, snapshot.total_requests),
            },
            latency: ReportLatency {
                min_ms: duration_to_ms(snapshot.histogram.min().unwrap_or(Duration::ZERO)),
                p50_ms: duration_to_ms(snapshot.percentiles.p50),
                p90_ms: duration_to_ms(snapshot.histogram.percentile(0.90)),
                p95_ms: duration_to_ms(snapshot.percentiles.p95),
                p99_ms: duration_to_ms(snapshot.percentiles.p99),
                max_ms: duration_to_ms(snapshot.histogram.max().unwrap_or(Duration::ZERO)),
            },
            latency_histogram: build_latency_histogram(&snapshot.latencies),
            status_codes: build_status_codes(snapshot),
            errors: build_errors(&snapshot.error_counts, snapshot.total_errors),
            timestamps_series: build_timestamps_series(snapshot),
            validation: build_validation_report(context.validation, snapshot),
        }
    }

    pub fn to_json_string(&self) -> String {
        serde_json::to_string_pretty(self).expect("HTML report data should serialize to JSON")
    }
}

fn build_latency_histogram(latencies: &[Duration]) -> Vec<LatencyHistogramBucket> {
    let mut buckets = [
        ("<1ms", 0_u64),
        ("1-5ms", 0_u64),
        ("5-10ms", 0_u64),
        ("10-25ms", 0_u64),
        ("25-50ms", 0_u64),
        ("50-100ms", 0_u64),
        ("100-250ms", 0_u64),
        ("250-500ms", 0_u64),
        ("500ms-1s", 0_u64),
        (">1s", 0_u64),
    ];

    for latency in latencies {
        let ms = duration_to_ms(*latency);

        let index = if ms < 1.0 {
            0
        } else if ms < 5.0 {
            1
        } else if ms < 10.0 {
            2
        } else if ms < 25.0 {
            3
        } else if ms < 50.0 {
            4
        } else if ms < 100.0 {
            5
        } else if ms < 250.0 {
            6
        } else if ms < 500.0 {
            7
        } else if ms <= 1_000.0 {
            8
        } else {
            9
        };

        buckets[index].1 += 1;
    }

    let total = latencies.len() as u64;

    buckets
        .into_iter()
        .map(|(bucket_label, count)| LatencyHistogramBucket {
            bucket_label: bucket_label.to_string(),
            count,
            percentage: percentage(count, total),
        })
        .collect()
}

fn build_status_codes(snapshot: &StatsSnapshot) -> Vec<StatusCodeReport> {
    snapshot
        .status_codes
        .iter()
        .map(|(code, count)| StatusCodeReport {
            code: *code,
            count: *count,
            percentage: percentage(*count, snapshot.total_requests),
        })
        .collect()
}

fn build_errors(error_counts: &ErrorCounts, total_errors: u64) -> Vec<ErrorReport> {
    [
        ("Timeout", error_counts.timeout),
        ("Connection", error_counts.connection),
        ("HTTP 4xx", error_counts.http_4xx),
        ("HTTP 5xx", error_counts.http_5xx),
        ("HTTP Other", error_counts.http_other),
        ("Other", error_counts.other),
    ]
    .into_iter()
    .filter(|(_, count)| *count > 0)
    .map(|(error_type, count)| ErrorReport {
        error_type: error_type.to_string(),
        count,
        percentage: percentage(count, total_errors),
    })
    .collect()
}

fn build_timestamps_series(snapshot: &StatsSnapshot) -> Vec<TimelinePoint> {
    vec![TimelinePoint {
        elapsed_secs: snapshot.duration.as_secs_f64(),
        rps: snapshot.throughput.requests_per_second,
        latency_p50: duration_to_ms(snapshot.percentiles.p50),
        latency_p99: duration_to_ms(snapshot.percentiles.p99),
    }]
}

fn build_validation_report(
    validation: Option<&ValidationConfig>,
    snapshot: &StatsSnapshot,
) -> Option<ValidationReport> {
    let has_expectations = validation.is_some_and(|validation| {
        validation.expect_status.is_some()
            || validation.expect_body.is_some()
            || validation
                .expect_headers
                .as_ref()
                .is_some_and(|headers| !headers.is_empty())
    });

    if !has_expectations && snapshot.errors.is_empty() {
        return None;
    }

    let expected_status = validation.and_then(|validation| validation.expect_status.clone());
    let expected_body = validation.and_then(|validation| validation.expect_body.clone());
    let expected_headers = validation
        .and_then(|validation| validation.expect_headers.as_ref())
        .map(|headers| {
            headers
                .iter()
                .map(|(key, value)| format!("{key}: {value}"))
                .collect()
        })
        .unwrap_or_default();

    Some(ValidationReport {
        expected_status,
        expected_body,
        expected_headers,
        failures: snapshot.errors.clone(),
    })
}

fn average_latency_ms(snapshot: &StatsSnapshot) -> f64 {
    if snapshot.latencies.is_empty() {
        return 0.0;
    }

    let total_ms = snapshot
        .latencies
        .iter()
        .map(|latency| duration_to_ms(*latency))
        .sum::<f64>();

    total_ms / snapshot.latencies.len() as f64
}

fn duration_to_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn percentage(value: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        value as f64 / total as f64 * 100.0
    }
}

fn unix_secs(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::{ErrorCategory, StatsCollector};

    fn sample_snapshot() -> StatsSnapshot {
        let mut stats = StatsCollector::new();

        stats.record(Duration::from_micros(500), 200);
        stats.record(Duration::from_millis(3), 200);
        stats.record(Duration::from_millis(7), 201);
        stats.record(Duration::from_millis(20), 404);
        stats.record(Duration::from_millis(40), 500);
        stats.record(Duration::from_millis(75), 200);
        stats.record(Duration::from_millis(200), 200);
        stats.record(Duration::from_millis(300), 200);
        stats.record(Duration::from_millis(750), 200);
        stats.record(Duration::from_millis(1_500), 200);
        stats.record_error(ErrorCategory::Timeout, "request timed out");

        stats.record_validation_error_with_retries(
            Duration::from_millis(90),
            200,
            "expected body to match pattern `ok`",
            2,
        );

        stats.snapshot()
    }

    fn context<'a>(
        snapshot: &'a StatsSnapshot,
        validation: Option<&'a ValidationConfig>,
    ) -> HtmlReportContext<'a> {
        HtmlReportContext {
            snapshot,
            target_url: "http://localhost:8000",
            command: "clank-cli http://localhost:8000 -n 10 -o html",
            rate_limit: None,
            generated_at: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
            app_version: "0.2.4",
            concurrency: 10,
            validation,
        }
    }

    #[test]
    fn html_report_data_converts_stats_snapshot() {
        let snapshot = sample_snapshot();
        let validation = ValidationConfig {
            expect_status: Some(vec![200]),
            expect_body: Some("ok".to_string()),
            expect_headers: Some(vec![(
                "Content-Type".to_string(),
                "application/json".to_string(),
            )]),
        };

        let data = HtmlReportData::from_stats_snapshot(&context(&snapshot, Some(&validation)));

        assert_eq!(data.metadata.timestamp, 1_700_000_000);
        assert_eq!(data.metadata.target_url, "http://localhost:8000");
        assert_eq!(data.metadata.concurrency, 10);
        assert_eq!(data.metadata.version, "0.2.4");
        assert_eq!(data.summary.total_requests, snapshot.total_requests);
        assert_eq!(data.summary.successful, snapshot.successful_requests);
        assert_eq!(data.summary.failed, snapshot.total_errors);
        assert_eq!(data.summary.validation_errors, snapshot.validation_errors);
        assert_eq!(data.latency_histogram.len(), 10);
        assert_eq!(data.status_codes.len(), 4);
        assert_eq!(data.timestamps_series.len(), 1);

        let validation_report = data.validation.expect("validation report should exist");
        assert_eq!(validation_report.expected_status, Some(vec![200]));
        assert_eq!(validation_report.expected_body, Some("ok".to_string()));
        assert_eq!(
            validation_report.expected_headers,
            vec!["Content-Type: application/json"]
        );
        assert_eq!(
            validation_report.failures,
            vec!["request timed out", "expected body to match pattern `ok`",]
        );
    }

    #[test]
    fn latency_histogram_uses_expected_buckets_and_percentages() {
        let latencies = vec![
            Duration::from_micros(500),
            Duration::from_millis(3),
            Duration::from_millis(7),
            Duration::from_millis(20),
            Duration::from_millis(40),
            Duration::from_millis(75),
            Duration::from_millis(200),
            Duration::from_millis(300),
            Duration::from_millis(750),
            Duration::from_millis(1_500),
        ];

        let buckets = build_latency_histogram(&latencies);
        let labels = buckets
            .iter()
            .map(|bucket| bucket.bucket_label.as_str())
            .collect::<Vec<_>>();
        let counts = buckets
            .iter()
            .map(|bucket| bucket.count)
            .collect::<Vec<_>>();

        assert_eq!(
            labels,
            vec![
                "<1ms",
                "1-5ms",
                "5-10ms",
                "10-25ms",
                "25-50ms",
                "50-100ms",
                "100-250ms",
                "250-500ms",
                "500ms-1s",
                ">1s"
            ]
        );
        assert_eq!(counts, vec![1, 1, 1, 1, 1, 1, 1, 1, 1, 1]);
        assert!(
            buckets
                .iter()
                .all(|bucket| (bucket.percentage - 10.0).abs() < 0.0001)
        );
    }

    #[test]
    fn status_codes_and_errors_include_percentages() {
        let snapshot = sample_snapshot();
        let data = HtmlReportData::from_stats_snapshot(&context(&snapshot, None));

        let status_200 = data
            .status_codes
            .iter()
            .find(|status| status.code == 200)
            .expect("status 200 should exist");

        assert_eq!(status_200.count, 8);
        assert!(status_200.percentage > 0.0);

        let timeout = data
            .errors
            .iter()
            .find(|error| error.error_type == "Timeout")
            .expect("timeout error should exist");

        assert_eq!(timeout.count, 1);
        assert!(timeout.percentage > 0.0);
    }

    #[test]
    fn html_report_data_serializes_to_browser_parseable_json() -> serde_json::Result<()> {
        let snapshot = sample_snapshot();
        let data = HtmlReportData::from_stats_snapshot(&context(&snapshot, None));

        let json = data.to_json_string();
        let value: serde_json::Value = serde_json::from_str(&json)?;

        assert_eq!(value["metadata"]["target_url"], "http://localhost:8000");
        assert_eq!(value["summary"]["total_requests"], snapshot.total_requests);
        assert!(value["latency_histogram"].is_array());
        assert!(value["timestamps_series"].is_array());

        Ok(())
    }
}
