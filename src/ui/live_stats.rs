use std::time::Duration;

use crate::config::RateLimitConfig;
use crate::ui::{ColorMode, error_rate_color, latency_color, maybe_color, throughput_color};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiveStats {
    pub elapsed: Duration,
    pub total_requests: u64,
    pub successful: u64,
    pub errors: u64,
    pub current_rps: f64,
    pub avg_latency_ms: f64,
    pub min_latency_ms: f64,
    pub max_latency_ms: f64,
}

impl LiveStats {
    pub fn calculate(
        elapsed: Duration,
        total_requests: u64,
        successful: u64,
        errors: u64,
        avg_latency_ms: f64,
        min_latency_ms: f64,
        max_latency_ms: f64,
    ) -> Self {
        let elapsed_secs = elapsed.as_secs_f64();

        let current_rps = if elapsed_secs > 0.0 {
            total_requests as f64 / elapsed_secs
        } else {
            0.0
        };

        Self {
            elapsed,
            total_requests,
            successful,
            errors,
            current_rps,
            avg_latency_ms,
            min_latency_ms,
            max_latency_ms,
        }
    }
}

impl Default for LiveStats {
    fn default() -> Self {
        Self {
            elapsed: Duration::ZERO,
            total_requests: 0,
            successful: 0,
            errors: 0,
            current_rps: 0.0,
            avg_latency_ms: 0.0,
            min_latency_ms: 0.0,
            max_latency_ms: 0.0,
        }
    }
}

pub fn format_live(stats: &LiveStats) -> String {
    format_live_with_color(stats, false)
}

pub fn format_live_with_color(stats: &LiveStats, color_enabled: bool) -> String {
    let error_percentage = if stats.total_requests == 0 {
        0.0
    } else {
        stats.errors as f64 / stats.total_requests as f64 * 100.0
    };

    let rps = maybe_color(
        &format!("{:.1} req/s", stats.current_rps),
        throughput_color(stats.current_rps),
        color_enabled,
    );

    let avg = maybe_color(
        &format!("avg {:.1}ms", stats.avg_latency_ms),
        latency_color(stats.avg_latency_ms),
        color_enabled,
    );

    let errors = maybe_color(
        &format!("{} errors", format_number(stats.errors)),
        error_rate_color(error_percentage),
        color_enabled,
    );

    format!(
        "{} req | {} | {} | {}",
        format_number(stats.total_requests),
        rps,
        avg,
        errors
    )
}

pub fn format_live_with_rate_limit_and_color(
    stats: &LiveStats,
    rate_limit: Option<&RateLimitConfig>,
    color_enabled: bool,
) -> String {
    let error_percentage = if stats.total_requests == 0 {
        0.0
    } else {
        stats.errors as f64 / stats.total_requests as f64 * 100.0
    };

    let rps = maybe_color(
        &format!("{:.1} req/s", stats.current_rps),
        throughput_color(stats.current_rps),
        color_enabled,
    );

    let rate_limit_text = format_rate_limit_live(stats, rate_limit);

    let rate_limit = maybe_color(
        &rate_limit_text,
        rate_limit_color(stats, rate_limit),
        color_enabled,
    );

    let avg = maybe_color(
        &format!("avg {:.1}ms", stats.avg_latency_ms),
        latency_color(stats.avg_latency_ms),
        color_enabled,
    );

    let errors = maybe_color(
        &format!("{} errors", format_number(stats.errors)),
        error_rate_color(error_percentage),
        color_enabled,
    );

    format!(
        "{} req | {} | {} | {} | {}",
        format_number(stats.total_requests),
        rps,
        rate_limit,
        avg,
        errors
    )
}

fn format_rate_limit_live(stats: &LiveStats, rate_limit: Option<&RateLimitConfig>) -> String {
    match rate_limit {
        Some(rate_limit) => format!(
            "Rate Limit: {} ({:.1}/{:.1} req/s)",
            rate_limit,
            stats.current_rps,
            rate_limit.requests_per_second()
        ),
        None => "Rate Limit: unlimited".to_string(),
    }
}

fn rate_limit_color(stats: &LiveStats, rate_limit: Option<&RateLimitConfig>) -> ColorMode {
    match rate_limit {
        Some(rate_limit) => {
            let limit = rate_limit.requests_per_second();

            if limit > 0.0 && stats.current_rps >= limit * 0.9 {
                ColorMode::Warning
            } else {
                ColorMode::Success
            }
        }
        None => ColorMode::Success,
    }
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

    #[test]
    fn live_stats_calculates_current_rps() {
        let stats = LiveStats::calculate(Duration::from_secs(10), 100, 80, 20, 45.0, 10.0, 90.0);

        assert_float_close(stats.current_rps, 10.0);
        assert_eq!(stats.total_requests, 100);
        assert_eq!(stats.successful, 80);
        assert_eq!(stats.errors, 20);
        assert_float_close(stats.avg_latency_ms, 45.0);
        assert_float_close(stats.min_latency_ms, 10.0);
        assert_float_close(stats.max_latency_ms, 90.0);
    }

    #[test]
    fn live_stats_handles_zero_duration() {
        let stats = LiveStats::calculate(Duration::ZERO, 100, 80, 20, 45.0, 10.0, 90.0);

        assert_float_close(stats.current_rps, 0.0);
    }

    #[test]
    fn live_stats_handles_zero_requests() {
        let stats = LiveStats::calculate(Duration::from_secs(10), 0, 0, 0, 0.0, 0.0, 0.0);

        assert_float_close(stats.current_rps, 0.0);
        assert_eq!(stats.total_requests, 0);
        assert_eq!(stats.errors, 0);
    }

    #[test]
    fn format_live_formats_compact_stats() {
        let stats =
            LiveStats::calculate(Duration::from_secs(10), 1_234, 1_200, 34, 45.2, 10.0, 100.0);

        let output = format_live(&stats);

        assert_eq!(output, "1,234 req | 123.4 req/s | avg 45.2ms | 34 errors");
    }

    #[test]
    fn format_live_with_color_disabled_matches_plain_output() {
        let stats =
            LiveStats::calculate(Duration::from_secs(10), 1_234, 1_200, 34, 45.2, 10.0, 100.0);

        assert_eq!(format_live(&stats), format_live_with_color(&stats, false));
    }

    #[test]
    fn format_live_with_rate_limit_includes_limit_and_actual_rps() {
        let stats = LiveStats::calculate(Duration::from_secs(10), 900, 900, 0, 45.2, 10.0, 100.0);
        let rate_limit = RateLimitConfig {
            rate: 100,
            period: crate::config::RatePeriod::Second,
        };

        let output = format_live_with_rate_limit_and_color(&stats, Some(&rate_limit), false);

        assert!(output.contains("Rate Limit: 100/s"));
        assert!(output.contains("90.0/100.0 req/s"));
    }

    #[test]
    fn format_live_with_rate_limit_shows_unlimited_when_missing() {
        let stats =
            LiveStats::calculate(Duration::from_secs(10), 1_234, 1_200, 34, 45.2, 10.0, 100.0);

        let output = format_live_with_rate_limit_and_color(&stats, None, false);

        assert!(output.contains("Rate Limit: unlimited"));
    }
}
