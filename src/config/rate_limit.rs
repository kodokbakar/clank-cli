use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Deserializer, de};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitConfig {
    pub rate: u64,
    pub period: RatePeriod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RatePeriod {
    Second,
    Minute,
    Hour,
}

impl RateLimitConfig {
    pub fn validate(&self) -> Result<()> {
        if self.rate == 0 {
            bail!("rate limit must be greater than 0");
        }

        Ok(())
    }

    pub fn interval(&self) -> Duration {
        self.period.duration()
    }

    pub fn requests_per_second(&self) -> f64 {
        self.rate as f64 / self.interval().as_secs_f64()
    }

    pub fn as_display_string(&self) -> String {
        format!("{}/{}", self.rate, self.period.suffix())
    }
}

impl RatePeriod {
    pub fn duration(&self) -> Duration {
        match self {
            Self::Second => Duration::from_secs(1),
            Self::Minute => Duration::from_secs(60),
            Self::Hour => Duration::from_secs(60 * 60),
        }
    }

    pub fn suffix(&self) -> &'static str {
        match self {
            Self::Second => "s",
            Self::Minute => "m",
            Self::Hour => "h",
        }
    }
}

impl fmt::Display for RateLimitConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.as_display_string())
    }
}

impl FromStr for RateLimitConfig {
    type Err = String;

    fn from_str(input: &str) -> std::result::Result<Self, Self::Err> {
        let input = input.trim();

        if input.is_empty() {
            return Err("rate limit cannot be empty".to_string());
        }

        let (rate, period) = input.split_once('/').ok_or_else(|| {
            format!("invalid rate limit format: {input}. Expected: <rate>/<s|m|h>")
        })?;

        let rate: u64 = rate
            .trim()
            .parse()
            .map_err(|_| format!("invalid rate limit value: {rate}"))?;

        let period = RatePeriod::from_str(period.trim())?;

        let config = Self { rate, period };

        config.validate().map_err(|error| error.to_string())?;

        Ok(config)
    }
}

impl FromStr for RatePeriod {
    type Err = String;

    fn from_str(input: &str) -> std::result::Result<Self, Self::Err> {
        match input.trim().to_ascii_lowercase().as_str() {
            "s" | "sec" | "second" | "seconds" => Ok(Self::Second),
            "m" | "min" | "minute" | "minutes" => Ok(Self::Minute),
            "h" | "hour" | "hours" => Ok(Self::Hour),
            _ => Err(format!(
                "unsupported rate limit period: {input}. Supported periods: s, m, h"
            )),
        }
    }
}

pub fn parse_rate_limit(input: &str) -> Result<RateLimitConfig> {
    input.parse().map_err(|error: String| anyhow!(error))
}

impl<'de> Deserialize<'de> for RateLimitConfig {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum RateLimitConfigDef {
            String(String),
            Object { rate: u64, period: RatePeriod },
        }

        let config = match RateLimitConfigDef::deserialize(deserializer)? {
            RateLimitConfigDef::String(value) => {
                RateLimitConfig::from_str(&value).map_err(de::Error::custom)?
            }
            RateLimitConfigDef::Object { rate, period } => RateLimitConfig { rate, period },
        };

        config.validate().map_err(de::Error::custom)?;

        Ok(config)
    }
}

impl<'de> Deserialize<'de> for RatePeriod {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let input = String::deserialize(deserializer)?;

        RatePeriod::from_str(&input).map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rate_limit_accepts_second_minute_and_hour() -> Result<()> {
        assert_eq!(
            parse_rate_limit("100/s")?,
            RateLimitConfig {
                rate: 100,
                period: RatePeriod::Second,
            }
        );

        assert_eq!(
            parse_rate_limit("5000/m")?,
            RateLimitConfig {
                rate: 5000,
                period: RatePeriod::Minute,
            }
        );

        assert_eq!(
            parse_rate_limit("10000/h")?,
            RateLimitConfig {
                rate: 10000,
                period: RatePeriod::Hour,
            }
        );

        Ok(())
    }

    #[test]
    fn parse_rate_limit_rejects_invalid_values() {
        assert!(parse_rate_limit("").is_err());
        assert!(parse_rate_limit("0/s").is_err());
        assert!(parse_rate_limit("abc/s").is_err());
        assert!(parse_rate_limit("100").is_err());
        assert!(parse_rate_limit("100/d").is_err());
    }

    #[test]
    fn rate_limit_config_deserializes_from_string() -> Result<()> {
        let config: RateLimitConfig = serde_yaml::from_str("10/s")?;

        assert_eq!(
            config,
            RateLimitConfig {
                rate: 10,
                period: RatePeriod::Second,
            }
        );

        Ok(())
    }

    #[test]
    fn rate_limit_config_deserializes_from_object() -> Result<()> {
        let config: RateLimitConfig = serde_yaml::from_str(
            r#"
rate: 60
period: minute
"#,
        )?;

        assert_eq!(
            config,
            RateLimitConfig {
                rate: 60,
                period: RatePeriod::Minute,
            }
        );

        Ok(())
    }

    #[test]
    fn rate_limit_config_rejects_zero_rate_from_object() -> Result<()> {
        let result = serde_yaml::from_str::<RateLimitConfig>(
            r#"
rate: 0
period: second
"#,
        );

        assert!(result.is_err());

        Ok(())
    }

    #[test]
    fn rate_limit_config_formats_display_string() {
        assert_eq!(
            RateLimitConfig {
                rate: 100,
                period: RatePeriod::Second,
            }
            .as_display_string(),
            "100/s"
        );

        assert_eq!(
            RateLimitConfig {
                rate: 5000,
                period: RatePeriod::Minute,
            }
            .as_display_string(),
            "5000/m"
        );

        assert_eq!(
            RateLimitConfig {
                rate: 10000,
                period: RatePeriod::Hour,
            }
            .as_display_string(),
            "10000/h"
        );
    }

    #[test]
    fn rate_limit_config_calculates_requests_per_second() {
        assert_eq!(
            RateLimitConfig {
                rate: 100,
                period: RatePeriod::Second,
            }
            .requests_per_second(),
            100.0
        );

        assert_eq!(
            RateLimitConfig {
                rate: 6000,
                period: RatePeriod::Minute,
            }
            .requests_per_second(),
            100.0
        );

        assert_eq!(
            RateLimitConfig {
                rate: 3600,
                period: RatePeriod::Hour,
            }
            .requests_per_second(),
            1.0
        );
    }
}
