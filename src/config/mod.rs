use std::time::Duration;

use anyhow::{Context, Result, bail};

pub fn parse_duration(input: &str) -> Result<Duration> {
    let input = input.trim();

    if input.is_empty() {
        bail!("duration cannot be empty");
    }

    let mut chars = input.chars().peekable();
    let mut total_secs = 0u64;
    let mut has_component = false;

    while chars.peek().is_some() {
        let mut number = String::new();

        while let Some(char) = chars.peek() {
            if char.is_ascii_digit() {
                number.push(*char);
                chars.next();
            } else {
                break;
            }
        }

        if number.is_empty() {
            bail!("invalid duration format: {input}");
        }

        let value: u64 = number
            .parse()
            .with_context(|| format!("invalid duration number: {number}"))?;

        let unit = chars
            .next()
            .with_context(|| format!("missing duration unit in: {input}"))?;

        let multiplier = match unit {
            's' => 1,
            'm' => 60,
            'h' => 60 * 60,
            _ => bail!("unsupported duration unit: {unit}"),
        };

        let component_secs = value
            .checked_mul(multiplier)
            .context("duration is too large")?;

        total_secs = total_secs
            .checked_add(component_secs)
            .context("duration is too large")?;

        has_component = true;
    }

    if !has_component || total_secs == 0 {
        bail!("duration must be greater than 0");
    }

    Ok(Duration::from_secs(total_secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_supports_seconds() -> Result<()> {
        assert_eq!(parse_duration("5s")?, Duration::from_secs(5));
        assert_eq!(parse_duration("30s")?, Duration::from_secs(30));

        Ok(())
    }

    #[test]
    fn parse_duration_supports_minutes() -> Result<()> {
        assert_eq!(parse_duration("5m")?, Duration::from_secs(300));

        Ok(())
    }

    #[test]
    fn parse_duration_supports_hours() -> Result<()> {
        assert_eq!(parse_duration("1h")?, Duration::from_secs(3600));

        Ok(())
    }

    #[test]
    fn parse_duration_supports_composite_duration() -> Result<()> {
        assert_eq!(parse_duration("1h30m")?, Duration::from_secs(5400));
        assert_eq!(parse_duration("1h30m5s")?, Duration::from_secs(5405));

        Ok(())
    }

    #[test]
    fn parse_duration_rejects_invalid_input() {
        assert!(parse_duration("").is_err());
        assert!(parse_duration("abc").is_err());
        assert!(parse_duration("5").is_err());
        assert!(parse_duration("5d").is_err());
        assert!(parse_duration("0s").is_err());
    }
}
