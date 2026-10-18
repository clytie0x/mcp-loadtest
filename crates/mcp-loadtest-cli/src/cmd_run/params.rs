//! Generic TOML-param plucking + duration parsing for the scenario builder.
//!
//! These are the type-coercion helpers shared by [`super::builder`] and
//! [`super::patterns`]: pull a typed value out of the untyped `params` blob
//! produced by the TOML config, erroring with a human-readable
//! `scenario.<field>` message on the wrong shape.

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde_json::Value;

/// Extract a required string field from a `params` blob, erroring with a
/// human-readable message if missing or the wrong shape.
pub(crate) fn required_str(params: &Value, key: &str) -> Result<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("scenario.{key} (string) is required"))
}

/// True when the config carries any of the pattern-style keys (`patterns`,
/// `tool_call`, `tool_calls`) — i.e. it should drive the weighted-pattern
/// engine instead of the single-`tool` path.
pub(crate) fn has_pattern_config(params: &Value) -> bool {
    params.get("patterns").is_some()
        || params.get("tool_call").is_some()
        || params.get("tool_calls").is_some()
}

/// Read an optional `u32` field, falling back to `default` when absent.
pub(crate) fn u32_field(params: &Value, key: &str, default: u32) -> Result<u32> {
    match params.get(key) {
        Some(v) => value_as_u32(v, key),
        None => Ok(default),
    }
}

fn value_as_u32(v: &Value, key: &str) -> Result<u32> {
    let n = v
        .as_u64()
        .ok_or_else(|| anyhow!("scenario.{key} must be a non-negative integer"))?;
    u32::try_from(n).with_context(|| format!("scenario.{key} exceeds u32::MAX"))
}

/// Parse a `params.<field>` slot as a humantime duration string, falling
/// back to `default` if the slot is absent and erroring on wrong types.
pub(crate) fn parse_dur_field(v: Option<&Value>, default: Duration) -> Result<Duration> {
    match v {
        Some(Value::String(s)) => parse_dur_str(s),
        None => Ok(default),
        _ => Err(anyhow!("duration must be a string like '60s'")),
    }
}

/// Parse a CLI / TOML duration string (`"60s"`, `"500ms"`, ...) into a
/// [`std::time::Duration`]. Wraps `humantime::parse_duration` with context.
pub fn parse_dur_str(s: &str) -> Result<Duration> {
    humantime::parse_duration(s).with_context(|| format!("parsing duration {s:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn required_str_errors_when_missing() {
        let err = required_str(&json!({}), "tool").unwrap_err();
        assert!(err.to_string().contains("scenario.tool"));
    }

    #[test]
    fn u32_field_falls_back_to_default() {
        assert_eq!(u32_field(&json!({}), "concurrent", 7).unwrap(), 7);
        assert_eq!(
            u32_field(&json!({"concurrent": 3}), "concurrent", 7).unwrap(),
            3
        );
    }

    #[test]
    fn value_as_u32_rejects_overflow_and_negatives() {
        assert!(value_as_u32(&json!(-1), "x").is_err());
        assert!(value_as_u32(&json!(u64::from(u32::MAX) + 1), "x").is_err());
    }

    #[test]
    fn parse_dur_field_defaults_and_type_checks() {
        assert_eq!(
            parse_dur_field(None, Duration::from_secs(5)).unwrap(),
            Duration::from_secs(5)
        );
        assert_eq!(
            parse_dur_field(Some(&json!("250ms")), Duration::ZERO).unwrap(),
            Duration::from_millis(250)
        );
        assert!(parse_dur_field(Some(&json!(5)), Duration::ZERO).is_err());
    }
}
