use crate::report::data::{HtmlReportContext, HtmlReportData};

const TEMPLATE: &str = include_str!("template.html");
const CHART_JS: &str = include_str!("chart.umd.min.js");
const CHART_JS_VERSION: &str = "4.5.1";

pub fn render_html_report(context: &HtmlReportContext<'_>) -> String {
    let data = HtmlReportData::from_stats_snapshot(context);
    let data_json = data.to_json_string();

    TEMPLATE
        .replace("{{CHART_JS}}", CHART_JS)
        .replace("{{REPORT_DATA}}", &escape_script_json(&data_json))
        .replace("{{APP_VERSION}}", context.app_version)
        .replace("{{CHART_JS_VERSION}}", CHART_JS_VERSION)
}

fn escape_script_json(json: &str) -> String {
    json.replace("</", "<\\/")
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;
    use crate::engine::ValidationConfig;
    use crate::stats::{ErrorCategory, StatsCollector, StatsSnapshot};

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
        let validation = ValidationConfig {
            expect_status: Some(vec![200]),
            expect_body: Some("ok".to_string()),
            expect_headers: Some(vec![(
                "Content-Type".to_string(),
                "application/json".to_string(),
            )]),
        };
        let context = HtmlReportContext {
            snapshot: &snapshot,
            target_url: "http://localhost:8000",
            command: "clank-cli http://localhost:8000 -n 100 -o html",
            rate_limit: None,
            generated_at: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
            app_version: "0.2.4",
            concurrency: 10,
            validation: Some(&validation),
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

        assert!(html.len() > TEMPLATE.len());

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
            concurrency: 1,
            validation: None,
        };

        let html = render_html_report(&context);

        assert!(html.contains("<\\/script>"));
        assert!(!html.contains("</script><script>alert(1)<\\/script>"));
    }
}
