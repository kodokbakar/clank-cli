use console::style;

pub type ColoredString = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Success,
    Warning,
    Error,
    Neutral,
}

pub fn success(text: &str) -> ColoredString {
    style(text).green().to_string()
}

pub fn error(text: &str) -> ColoredString {
    style(text).red().to_string()
}

pub fn warning(text: &str) -> ColoredString {
    style(text).yellow().to_string()
}

pub fn dim(text: &str) -> ColoredString {
    style(text).dim().to_string()
}

pub fn bold(text: &str) -> ColoredString {
    style(text).bold().to_string()
}

pub fn maybe_color(text: &str, mode: ColorMode, enabled: bool) -> String {
    if !enabled {
        return text.to_string();
    }

    match mode {
        ColorMode::Success => success(text),
        ColorMode::Warning => warning(text),
        ColorMode::Error => error(text),
        ColorMode::Neutral => text.to_string(),
    }
}

pub fn success_rate_color(success_percentage: f64) -> ColorMode {
    if success_percentage > 95.0 {
        ColorMode::Success
    } else if success_percentage >= 80.0 {
        ColorMode::Warning
    } else {
        ColorMode::Error
    }
}

pub fn error_rate_color(error_percentage: f64) -> ColorMode {
    if error_percentage > 0.0 {
        ColorMode::Error
    } else {
        ColorMode::Success
    }
}

pub fn latency_color(latency_ms: f64) -> ColorMode {
    if latency_ms > 500.0 {
        ColorMode::Error
    } else if latency_ms > 200.0 {
        ColorMode::Warning
    } else {
        ColorMode::Success
    }
}

pub fn throughput_color(requests_per_second: f64) -> ColorMode {
    if requests_per_second > 50.0 {
        ColorMode::Success
    } else if requests_per_second > 10.0 {
        ColorMode::Warning
    } else {
        ColorMode::Neutral
    }
}

pub fn count_error_color(count: u64) -> ColorMode {
    if count > 0 {
        ColorMode::Error
    } else {
        ColorMode::Neutral
    }
}

pub fn count_warning_color(count: u64) -> ColorMode {
    if count > 0 {
        ColorMode::Warning
    } else {
        ColorMode::Neutral
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_rate_thresholds_match_expected_colors() {
        assert_eq!(success_rate_color(96.0), ColorMode::Success);
        assert_eq!(success_rate_color(95.0), ColorMode::Warning);
        assert_eq!(success_rate_color(80.0), ColorMode::Warning);
        assert_eq!(success_rate_color(79.9), ColorMode::Error);
    }

    #[test]
    fn error_rate_thresholds_match_expected_colors() {
        assert_eq!(error_rate_color(0.0), ColorMode::Success);
        assert_eq!(error_rate_color(0.1), ColorMode::Error);
    }

    #[test]
    fn latency_thresholds_match_expected_colors() {
        assert_eq!(latency_color(100.0), ColorMode::Success);
        assert_eq!(latency_color(200.0), ColorMode::Success);
        assert_eq!(latency_color(200.1), ColorMode::Warning);
        assert_eq!(latency_color(500.0), ColorMode::Warning);
        assert_eq!(latency_color(500.1), ColorMode::Error);
    }

    #[test]
    fn throughput_thresholds_match_expected_colors() {
        assert_eq!(throughput_color(50.1), ColorMode::Success);
        assert_eq!(throughput_color(50.0), ColorMode::Warning);
        assert_eq!(throughput_color(10.1), ColorMode::Warning);
        assert_eq!(throughput_color(10.0), ColorMode::Neutral);
    }

    #[test]
    fn maybe_color_returns_plain_text_when_disabled() {
        assert_eq!(maybe_color("hello", ColorMode::Error, false), "hello");
    }

    #[test]
    fn color_helpers_return_text_content() {
        assert!(success("ok").contains("ok"));
        assert!(error("bad").contains("bad"));
        assert!(warning("warn").contains("warn"));
        assert!(dim("dimmed").contains("dimmed"));
        assert!(bold("strong").contains("strong"));
    }
}
