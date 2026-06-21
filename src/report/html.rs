use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::config::RateLimitConfig;
use crate::stats::{ErrorCounts, StatsSnapshot};

const TEMPLATE: &str = include_str!("template.html");
const CHART_JS: &str = include_str!("chart.umd.min.js");
const CHART_JS_VERSION: &str = "4.5.1";

#[derive(Debug, Clone)]
pub struct HtmlReportContext<'a> {
    pub snapshot: &'a StatsSnapshot,
    pub target_url: &'a str,
    pub command: &'a str,
    pub rate_limit: Option<&'a RateLimitConfig>,
    pub generated_at: SystemTime,
    pub app_version: &'a str,
}

pub fn render_html_report(context: &HtmlReportContext<'_>) -> String {
    let data = ReportData::from_context(context);
    let data_json =
        serde_json::to_string_pretty(&data).expect("HTML report data should serialize to JSON");

    TEMPLATE
        .replace("{{CHART_JS}}", CHART_JS)
        .replace("{{REPORT_DATA}}", &escape_script_json(&data_json))
        .replace("{{APP_VERSION}}", context.app_version)
        .replace("{{CHART_JS_VERSION}}", CHART_JS_VERSION)
}

#[derive(Debug, Serialize)]
struct ReportData {
    app_name: &'static str,
    app_version: String,
    chart_js_version: String,
    generated_at_unix_secs: u64,
    generated_at_display: String,
    target_url: String,
    command: String,
    duration_secs: f64,
    total_requests: u64,
    successful_requests: u64,
    total_errors: u64,
    validation_errors: usize,
    retries: usize,
    success_rate: f64,
    error_rate: f64,
    throughput_rps: f64,
    rate_limit: String,
    latency: LatencyData,
    latency_buckets: Vec<ChartPoint>,
    status_codes: Vec<ChartPoint>,
    error_breakdown: Vec<ChartPoint>,
    validation: ValidationData,
}

#[derive(Debug, Serialize)]
struct LatencyData {
    min_ms: f64,
    avg_ms: f64,
    p50_ms: f64,
    p90_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    max_ms: f64,
}

#[derive(Debug, Serialize)]
struct ChartPoint {
    label: String,
    value: u64,
}

#[derive(Debug, Serialize)]
struct ValidationData {
    has_validation_errors: bool,
    count: usize,
    errors: Vec<String>,
}

impl ReportData {
    fn from_context(context: &HtmlReportContext<'_>) -> Self {
        let snapshot = context.snapshot;
        let generated_at_unix_secs = unix_secs(context.generated_at);

        Self {
            app_name: "clank-cli",
            app_version: context.app_version.to_string(),
            chart_js_version: CHART_JS_VERSION.to_string(),
            generated_at_unix_secs,
            generated_at_display: format!("{generated_at_unix_secs}"),
            target_url: context.target_url.to_string(),
            command: context.command.to_string(),
            duration_secs: snapshot.duration.as_secs_f64(),
            total_requests: snapshot.total_requests,
            successful_requests: snapshot.successful_requests,
            total_errors: snapshot.total_errors,
            validation_errors: snapshot.validation_errors,
            retries: snapshot.retries,
            success_rate: percentage(snapshot.successful_requests, snapshot.total_requests),
            error_rate: percentage(snapshot.total_errors, snapshot.total_requests),
            throughput_rps: snapshot.throughput.requests_per_second,
            rate_limit: context
                .rate_limit
                .map(RateLimitConfig::as_display_string)
                .unwrap_or_else(|| "unlimited".to_string()),
            latency: latency_data(snapshot),
            latency_buckets: latency_buckets(&snapshot.latencies),
            status_codes: status_code_points(&snapshot.status_codes),
            error_breakdown: error_breakdown_points(&snapshot.error_counts),
            validation: ValidationData {
                has_validation_errors: snapshot.validation_errors > 0,
                count: snapshot.validation_errors,
                errors: snapshot.errors.clone(),
            },
        }
    }
}

fn latency_data(snapshot: &StatsSnapshot) -> LatencyData {
    LatencyData {
        min_ms: duration_to_ms(snapshot.histogram.min().unwrap_or(Duration::ZERO)),
        avg_ms: average_latency_ms(snapshot),
        p50_ms: duration_to_ms(snapshot.percentiles.p50),
        p90_ms: duration_to_ms(snapshot.histogram.percentile(0.90)),
        p95_ms: duration_to_ms(snapshot.percentiles.p95),
        p99_ms: duration_to_ms(snapshot.percentiles.p99),
        max_ms: duration_to_ms(snapshot.histogram.max().unwrap_or(Duration::ZERO)),
    }
}

fn latency_buckets(latencies: &[Duration]) -> Vec<ChartPoint> {
    let mut buckets = [
        ("0-50ms", 0_u64),
        ("50-100ms", 0_u64),
        ("100-250ms", 0_u64),
        ("250-500ms", 0_u64),
        ("500ms-1s", 0_u64),
        (">1s", 0_u64),
    ];

    for latency in latencies {
        let ms = duration_to_ms(*latency);

        let index = if ms <= 50.0 {
            0
        } else if ms <= 100.0 {
            1
        } else if ms <= 250.0 {
            2
        } else if ms <= 500.0 {
            3
        } else if ms <= 1_000.0 {
            4
        } else {
            5
        };

        buckets[index].1 += 1;
    }

    buckets
        .into_iter()
        .map(|(label, value)| ChartPoint {
            label: label.to_string(),
            value,
        })
        .collect()
}

fn status_code_points(status_codes: &BTreeMap<u16, u64>) -> Vec<ChartPoint> {
    status_codes
        .iter()
        .map(|(status, count)| ChartPoint {
            label: status.to_string(),
            value: *count,
        })
        .collect()
}

fn error_breakdown_points(error_counts: &ErrorCounts) -> Vec<ChartPoint> {
    [
        ("Timeout", error_counts.timeout),
        ("Connection", error_counts.connection),
        ("HTTP 4xx", error_counts.http_4xx),
        ("HTTP 5xx", error_counts.http_5xx),
        ("HTTP Other", error_counts.http_other),
        ("Other", error_counts.other),
    ]
    .into_iter()
    .filter(|(_, value)| *value > 0)
    .map(|(label, value)| ChartPoint {
        label: label.to_string(),
        value,
    })
    .collect()
}

fn percentage(value: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        value as f64 / total as f64 * 100.0
    }
}

fn average_latency_ms(snapshot: &StatsSnapshot) -> f64 {
    if snapshot.latencies.is_empty() {
        return 0.0;
    }

    let total_ms: f64 = snapshot
        .latencies
        .iter()
        .map(|latency| duration_to_ms(*latency))
        .sum();

    total_ms / snapshot.latencies.len() as f64
}

fn duration_to_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn unix_secs(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

fn escape_script_json(json: &str) -> String {
    json.replace("</", "<\\/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::{ErrorCategory, StatsCollector};

    fn sample_snapshot() -> StatsSnapshot {
        let mut stats = StatsCollector::new();

        for _ in 0..80 {
            stats.record(Duration::from_millis(40), 200);
        }

        for _ in 0..10 {
            stats.record(Duration::from_millis(120), 404);
        }

        for _ in 0..5 {
            stats.record_error(ErrorCategory::Timeout, "request timed out");
        }

        stats.record_validation_error_with_retries(
            Duration::from_millis(90),
            200,
            "expected body to match pattern `ok`",
            1,
        );

        stats.snapshot()
    }

    #[test]
    fn render_html_report_outputs_self_contained_document() {
        let snapshot = sample_snapshot();
        let context = HtmlReportContext {
            snapshot: &snapshot,
            target_url: "http://localhost:8000",
            command: "clank-cli http://localhost:8000 -n 100 -o html",
            rate_limit: None,
            generated_at: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
            app_version: "0.2.4",
        };

        let html = render_html_report(&context);

        assert!(html.contains("<!doctype html>"));
        assert!(html.contains("clank-cli Load Test Report"));
        assert!(html.contains("http://localhost:8000"));
        assert!(html.contains("Chart.js v4.5.1"));
        assert!(html.contains("new Chart("));
        assert!(html.contains("Chart.defaults"));
        assert!(html.contains("application/json"));
        assert!(html.contains("expected body to match pattern"));

        let chart_bundle_marker = &CHART_JS[..CHART_JS.len().min(80)];
        assert!(html.contains(chart_bundle_marker));

        assert!(!html.contains("src=\"http://"));
        assert!(!html.contains("src=\"https://"));
        assert!(!html.contains("href=\"http://"));
        assert!(!html.contains("href=\"https://"));
        assert!(!html.contains("url(http://"));
        assert!(!html.contains("url(https://"));
        assert!(!html.contains("@import"));
        assert!(!html.contains("{{REPORT_DATA}}"));
        assert!(!html.contains("{{CHART_JS}}"));
        assert!(!html.contains("{ { CHART_JS } }"));
    }

    #[test]
    fn render_html_report_escapes_script_breakout_sequence() {
        let mut stats = StatsCollector::new();

        stats.record_validation_error_with_retries(
            Duration::from_millis(10),
            200,
            "</script><script>alert(1)</script>",
            0,
        );

        let snapshot = stats.snapshot();
        let context = HtmlReportContext {
            snapshot: &snapshot,
            target_url: "http://localhost:8000",
            command: "clank-cli http://localhost:8000 --expect-body '</script>'",
            rate_limit: None,
            generated_at: UNIX_EPOCH,
            app_version: "0.2.4",
        };

        let html = render_html_report(&context);

        assert!(html.contains("<\\/script>"));
        assert!(!html.contains("</script><script>alert(1)<\\/script>"));
    }

    #[test]
    fn latency_buckets_include_all_latency_samples() {
        let latencies = vec![
            Duration::from_millis(10),
            Duration::from_millis(75),
            Duration::from_millis(200),
            Duration::from_millis(300),
            Duration::from_millis(750),
            Duration::from_millis(1_500),
        ];

        let buckets = latency_buckets(&latencies);
        let total = buckets.iter().map(|bucket| bucket.value).sum::<u64>();

        assert_eq!(total, 6);
    }
}
