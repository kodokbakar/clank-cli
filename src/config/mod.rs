use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

pub const DEFAULT_CONFIG_FILE: &str = "clank.yaml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    #[default]
    Text,
    Json,
    Csv,
}

impl FromStr for OutputFormat {
    type Err = String;

    fn from_str(input: &str) -> std::result::Result<Self, Self::Err> {
        match input.trim().to_ascii_lowercase().as_str() {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            "csv" => Ok(Self::Csv),
            _ => Err(format!(
                "unsupported output format: {input}. Supported formats: text, json, csv"
            )),
        }
    }
}

pub fn parse_output_format(input: &str) -> Result<OutputFormat> {
    input.parse().map_err(|error: String| anyhow!(error))
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ClankConfig {
    pub url: String,
    #[serde(default = "default_method")]
    pub method: String,
    pub body: Option<String>,
    #[serde(default)]
    pub body_file: Option<PathBuf>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub headers: Vec<String>,
    #[serde(default)]
    pub insecure: bool,
    #[serde(default)]
    pub output: Option<String>,
}

impl ClankConfig {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();

        let contents = fs::read_to_string(path)
            .with_context(|| format!("failed to read config file: {}", path.display()))?;

        Self::from_yaml_str(path, &contents)
    }

    pub fn optional_from_file(path: impl AsRef<Path>) -> Result<Option<Self>> {
        let path = path.as_ref();

        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to read config file: {}", path.display()));
            }
        };

        Self::from_yaml_str(path, &contents).map(Some)
    }

    pub fn validate(&self) -> Result<()> {
        if self.url.trim().is_empty() {
            bail!("config url cannot be empty");
        }

        validate_method(&self.method).with_context(|| "invalid config method")?;

        if self.body.is_some() && self.body_file.is_some() {
            bail!("config body and body_file cannot be used together");
        }

        if let Some(body_file) = &self.body_file
            && body_file.as_os_str().is_empty()
        {
            bail!("config body_file cannot be empty");
        }

        if let Some(content_type) = &self.content_type
            && content_type.trim().is_empty()
        {
            bail!("config content_type cannot be empty");
        }

        if self.concurrency == 0 {
            bail!("config concurrency must be greater than 0");
        }

        if self.timeout_secs == 0 {
            bail!("config timeout_secs must be greater than 0");
        }

        for header in &self.headers {
            parse_header(header).with_context(|| format!("invalid config header: {header}"))?;
        }

        if let Some(output) = &self.output {
            parse_output_format(output).with_context(|| "invalid config output format")?;
        }

        Ok(())
    }

    fn from_yaml_str(path: &Path, contents: &str) -> Result<Self> {
        let config: Self = serde_yaml::from_str(contents)
            .with_context(|| format!("failed to parse YAML config file: {}", path.display()))?;

        config.validate()?;

        Ok(config)
    }
}

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

pub fn validate_method(method: &str) -> Result<String> {
    let normalized = method.trim().to_ascii_uppercase();

    if normalized.is_empty() {
        bail!("HTTP method cannot be empty");
    }

    match normalized.as_str() {
        "GET" | "POST" | "PUT" | "DELETE" | "PATCH" | "HEAD" | "OPTIONS" => Ok(normalized),
        _ => bail!("unsupported method: {normalized}"),
    }
}

pub fn parse_header(input: &str) -> Result<(String, String)> {
    let (key, value) = input
        .split_once(':')
        .with_context(|| format!("invalid header format: {input}. Expected: Key: Value"))?;

    let key = key.trim();
    let value = value.trim();

    if key.is_empty() {
        bail!("header key cannot be empty: {input}");
    }

    Ok((key.to_string(), value.to_string()))
}

fn default_method() -> String {
    "GET".to_string()
}

fn default_concurrency() -> usize {
    10
}

fn default_timeout() -> u64 {
    10
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_config_path(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after UNIX epoch")
            .as_nanos();

        std::env::temp_dir().join(format!("clank-cli-{name}-{nanos}.yaml"))
    }

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

    #[test]
    fn validate_method_accepts_supported_methods() -> Result<()> {
        assert_eq!(validate_method("GET")?, "GET");
        assert_eq!(validate_method("post")?, "POST");
        assert_eq!(validate_method(" Put ")?, "PUT");
        assert_eq!(validate_method("DELETE")?, "DELETE");
        assert_eq!(validate_method("PATCH")?, "PATCH");
        assert_eq!(validate_method("HEAD")?, "HEAD");
        assert_eq!(validate_method("OPTIONS")?, "OPTIONS");

        Ok(())
    }

    #[test]
    fn validate_method_rejects_unsupported_methods() {
        let error = validate_method("trace").unwrap_err();

        assert_eq!(error.to_string(), "unsupported method: TRACE");
        assert!(validate_method("").is_err());
        assert!(validate_method("CONNECT").is_err());
    }

    #[test]
    fn parse_header_accepts_key_value_format() -> Result<()> {
        let header = parse_header("Authorization: Bearer token123")?;

        assert_eq!(header.0, "Authorization");
        assert_eq!(header.1, "Bearer token123");

        Ok(())
    }

    #[test]
    fn parse_header_trims_key_and_value() -> Result<()> {
        let header = parse_header(" Content-Type : application/json ")?;

        assert_eq!(header.0, "Content-Type");
        assert_eq!(header.1, "application/json");

        Ok(())
    }

    #[test]
    fn parse_header_rejects_invalid_input() {
        assert!(parse_header("").is_err());
        assert!(parse_header("Authorization").is_err());
        assert!(parse_header(": value").is_err());
    }

    #[test]
    fn clank_config_parses_yaml_file() -> Result<()> {
        let path = unique_config_path("valid");

        fs::write(
            &path,
            r#"
url: http://localhost:3000/api
method: POST
body: '{"name":"test"}'
content_type: application/json
concurrency: 20
timeout_secs: 30
headers:
  - "Authorization: Bearer token123"
  - "Content-Type: text/plain"
insecure: true
"#,
        )?;

        let config = ClankConfig::from_file(&path)?;

        assert_eq!(config.url, "http://localhost:3000/api");
        assert_eq!(config.method, "POST");
        assert_eq!(config.body, Some(r#"{"name":"test"}"#.to_string()));
        assert_eq!(config.content_type, Some("application/json".to_string()));
        assert_eq!(config.body_file, None);
        assert_eq!(config.concurrency, 20);
        assert_eq!(config.timeout_secs, 30);
        assert_eq!(config.headers.len(), 2);
        assert!(config.insecure);

        fs::remove_file(path)?;

        Ok(())
    }

    #[test]
    fn clank_config_uses_defaults() -> Result<()> {
        let config: ClankConfig = serde_yaml::from_str(
            r#"
url: http://localhost:3000/api
"#,
        )?;

        config.validate()?;

        assert_eq!(config.method, "GET");
        assert_eq!(config.body, None);
        assert_eq!(config.body_file, None);
        assert_eq!(config.content_type, None);
        assert_eq!(config.concurrency, 10);
        assert_eq!(config.timeout_secs, 10);
        assert!(config.headers.is_empty());
        assert!(!config.insecure);
        assert_eq!(config.output, None);

        Ok(())
    }

    #[test]
    fn optional_from_file_returns_none_when_file_is_missing() -> Result<()> {
        let path = unique_config_path("missing");

        let config = ClankConfig::optional_from_file(path)?;

        assert_eq!(config, None);

        Ok(())
    }

    #[test]
    fn clank_config_rejects_corrupt_yaml() -> Result<()> {
        let path = unique_config_path("corrupt");

        fs::write(&path, "url: [")?;

        let result = ClankConfig::optional_from_file(&path);

        assert!(result.is_err());

        fs::remove_file(path)?;

        Ok(())
    }

    #[test]
    fn clank_config_rejects_invalid_method() -> Result<()> {
        let config: ClankConfig = serde_yaml::from_str(
            r#"
url: http://localhost:3000/api
method: TRACE
"#,
        )?;

        assert!(config.validate().is_err());

        Ok(())
    }

    #[test]
    fn clank_config_rejects_invalid_header() -> Result<()> {
        let config: ClankConfig = serde_yaml::from_str(
            r#"
url: http://localhost:3000/api
headers:
  - "Authorization"
"#,
        )?;

        assert!(config.validate().is_err());

        Ok(())
    }

    #[test]
    fn clank_config_rejects_zero_concurrency() -> Result<()> {
        let config: ClankConfig = serde_yaml::from_str(
            r#"
url: http://localhost:3000/api
concurrency: 0
"#,
        )?;

        assert!(config.validate().is_err());

        Ok(())
    }

    #[test]
    fn clank_config_rejects_zero_timeout_secs() -> Result<()> {
        let config: ClankConfig = serde_yaml::from_str(
            r#"
url: http://localhost:3000/api
timeout_secs: 0
"#,
        )?;

        assert!(config.validate().is_err());

        Ok(())
    }

    #[test]
    fn output_format_accepts_supported_values() -> Result<()> {
        assert_eq!(parse_output_format("text")?, OutputFormat::Text);
        assert_eq!(parse_output_format("json")?, OutputFormat::Json);
        assert_eq!(parse_output_format("csv")?, OutputFormat::Csv);
        assert_eq!(parse_output_format("TEXT")?, OutputFormat::Text);
        assert_eq!(parse_output_format(" Json ")?, OutputFormat::Json);

        Ok(())
    }

    #[test]
    fn output_format_rejects_unsupported_values() {
        assert!(parse_output_format("").is_err());
        assert!(parse_output_format("xml").is_err());
        assert!(parse_output_format("table").is_err());
    }

    #[test]
    fn clank_config_accepts_output_format() -> Result<()> {
        let config: ClankConfig = serde_yaml::from_str(
            r#"
url: http://localhost:3000/api
output: json
"#,
        )?;

        config.validate()?;

        assert_eq!(config.output, Some("json".to_string()));

        Ok(())
    }

    #[test]
    fn clank_config_rejects_invalid_output_format() -> Result<()> {
        let config: ClankConfig = serde_yaml::from_str(
            r#"
url: http://localhost:3000/api
output: xml
"#,
        )?;

        assert!(config.validate().is_err());

        Ok(())
    }

    #[test]
    fn clank_config_accepts_body_file_and_content_type() -> Result<()> {
        let config: ClankConfig = serde_yaml::from_str(
            r#"
url: http://localhost:3000/api
method: POST
body_file: ./request_body.json
content_type: application/json
"#,
        )?;

        config.validate()?;

        assert_eq!(config.body, None);
        assert_eq!(config.body_file, Some(PathBuf::from("./request_body.json")));
        assert_eq!(config.content_type, Some("application/json".to_string()));

        Ok(())
    }

    #[test]
    fn clank_config_rejects_body_and_body_file_together() -> Result<()> {
        let config: ClankConfig = serde_yaml::from_str(
            r#"
url: http://localhost:3000/api
method: POST
body: '{"name":"test"}'
body_file: ./request_body.json
"#,
        )?;

        assert!(config.validate().is_err());

        Ok(())
    }

    #[test]
    fn clank_config_rejects_empty_content_type() -> Result<()> {
        let config: ClankConfig = serde_yaml::from_str(
            r#"
url: http://localhost:3000/api
content_type: " "
"#,
        )?;

        assert!(config.validate().is_err());

        Ok(())
    }
}
