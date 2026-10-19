//! Scenario `kind` → `Box<dyn Scenario>` dispatch.
//!
//! Pure function over the (untyped) TOML `params` blob — the single place
//! where a config's `scenario.kind` string is mapped to a concrete scenario,
//! plucking per-kind parameters via [`super::params`] / [`super::patterns`].

use std::time::Duration;

use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use mcp_loadtest::ProtocolVersion;
use mcp_loadtest::scenario::Scenario;
use mcp_loadtest::scenario::cold_start::ColdStart;
use mcp_loadtest::scenario::deadlock_probe::DeadlockProbe;
use mcp_loadtest::scenario::lost_update::LostUpdate;
use mcp_loadtest::scenario::pattern::PatternScenario;
use mcp_loadtest::scenario::race_check::RaceCheck;
use mcp_loadtest::scenario::sustained::Sustained;
use mcp_loadtest::scenario::version_matrix::VersionMatrix;

use super::params::{has_pattern_config, parse_dur_field, required_str, u32_field};
use super::patterns::parse_patterns;

/// Dispatch on the scenario `kind` string from a TOML config, plucking
/// per-kind parameters out of the (untyped) `params` blob.
pub(crate) fn build_scenario(kind: &str, params: &Value) -> Result<Box<dyn Scenario>> {
    match kind {
        "sustained" => {
            let concurrent = positive_u32("concurrent", u32_field(params, "concurrent", 10)?)?;
            let duration = positive_duration(
                "duration",
                parse_dur_field(params.get("duration"), Duration::from_secs(60))?,
            )?;
            if has_pattern_config(params) {
                let patterns = parse_patterns(params)?;
                return Ok(Box::new(PatternScenario::sustained(
                    concurrent, duration, patterns,
                )));
            }
            let tool = required_str(params, "tool")?;
            let args = params.get("args").cloned().unwrap_or(json!({}));
            Ok(Box::new(Sustained {
                concurrent,
                duration,
                tool,
                args,
            }))
        }
        "deadlock_probe" => {
            let concurrent = positive_u32("concurrent", u32_field(params, "concurrent", 20)?)?;
            let hang_threshold = positive_duration(
                "hang_threshold",
                parse_dur_field(params.get("hang_threshold"), Duration::from_secs(5))?,
            )?;
            let grace_period =
                parse_dur_field(params.get("grace_period"), Duration::from_secs(10))?;
            let tool = required_str(params, "tool")?;
            let args = params.get("args").cloned().unwrap_or(json!({}));
            Ok(Box::new(DeadlockProbe {
                concurrent,
                hang_threshold,
                grace_period,
                tool,
                args,
            }))
        }
        "cold_start" => {
            let iterations = positive_u32("iterations", u32_field(params, "iterations", 5)?)?;
            let warmup = params
                .get("warmup")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            if warmup && iterations < 2 {
                return Err(anyhow!(
                    "scenario.iterations must be >= 2 when scenario.warmup = true; \
                     the warm-up iteration is excluded from measured evidence"
                ));
            }
            let tool = required_str(params, "tool")?;
            let args = params.get("args").cloned().unwrap_or(json!({}));
            Ok(Box::new(ColdStart {
                iterations,
                warmup,
                tool,
                args,
            }))
        }
        "race_check" => {
            let concurrent = u32_field(params, "concurrent", 10)?;
            if concurrent < 2 {
                return Err(anyhow!(
                    "scenario.concurrent: race_check requires at least 2 synchronized calls"
                ));
            }
            let tool = required_str(params, "tool")?;
            let args = params.get("args").cloned().unwrap_or(json!({}));
            Ok(Box::new(RaceCheck {
                concurrent,
                tool,
                args,
            }))
        }
        "lost_update" => {
            let plan = LostUpdate {
                concurrent: u32_field(params, "concurrent", 10)?,
                writes_per_worker: u32_field(params, "writes_per_worker", 5)?,
                write_tool: required_str(params, "write_tool")?,
                write_args: params
                    .get("write_args")
                    .cloned()
                    .ok_or_else(|| anyhow!("scenario.write_args (object) is required"))?,
                read_tool: required_str(params, "read_tool")?,
                read_args: params.get("read_args").cloned().unwrap_or(json!({})),
                pipelined: match params.get("pipelined") {
                    None => false,
                    Some(v) => v
                        .as_bool()
                        .ok_or_else(|| anyhow!("scenario.pipelined must be a boolean"))?,
                },
            };
            if let Some(message) = plan.validation_error() {
                return Err(anyhow!("scenario: {message}"));
            }
            Ok(Box::new(plan))
        }
        "pattern" => {
            let concurrent = positive_u32("concurrent", u32_field(params, "concurrent", 10)?)?;
            let duration = positive_duration(
                "duration",
                parse_dur_field(params.get("duration"), Duration::from_secs(60))?,
            )?;
            let patterns = parse_patterns(params)?;
            Ok(Box::new(PatternScenario::new(
                concurrent, duration, patterns,
            )))
        }
        "version_matrix" => {
            let calls_per_version = positive_u32(
                "calls_per_version",
                u32_field(params, "calls_per_version", 10)?,
            )?;
            let tool = required_str(params, "tool")?;
            let args = params.get("args").cloned().unwrap_or(json!({}));
            let versions = parse_versions(params)?;
            Ok(Box::new(VersionMatrix {
                versions,
                calls_per_version,
                tool,
                args,
            }))
        }
        other => Err(anyhow!("unknown scenario kind: {other}")),
    }
}

fn positive_u32(field: &str, value: u32) -> Result<u32> {
    if value == 0 {
        Err(anyhow!("scenario.{field} must be >= 1"))
    } else {
        Ok(value)
    }
}

fn positive_duration(field: &str, value: Duration) -> Result<Duration> {
    if value.is_zero() {
        Err(anyhow!("scenario.{field} must be > 0"))
    } else {
        Ok(value)
    }
}

/// Pluck the optional `versions` string array and parse each entry into a
/// [`ProtocolVersion`]. Empty / absent means "all supported revisions"
/// (resolved inside the scenario).
fn parse_versions(params: &Value) -> Result<Vec<ProtocolVersion>> {
    let Some(raw) = params.get("versions") else {
        return Ok(Vec::new());
    };
    let entries = raw
        .as_array()
        .ok_or_else(|| anyhow!("versions: expected an array of revision strings"))?;
    entries
        .iter()
        .map(|entry| {
            let s = entry
                .as_str()
                .ok_or_else(|| anyhow!("versions: entries must be strings, got {entry}"))?;
            ProtocolVersion::parse(s).ok_or_else(|| {
                let supported: Vec<&str> = ProtocolVersion::SUPPORTED
                    .iter()
                    .map(|v| v.as_str())
                    .collect();
                anyhow!(
                    "versions: unsupported revision `{s}` (expected one of: {})",
                    supported.join(", ")
                )
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcp_loadtest::config::Config;

    #[test]
    fn build_scenario_supports_every_validated_kind() {
        let cases = [
            (
                "sustained",
                json!({"tool": "echo", "duration": "1s"}),
                "sustained",
            ),
            (
                "deadlock_probe",
                json!({"tool": "echo", "hang_threshold": "10ms", "grace_period": "10ms"}),
                "deadlock_probe",
            ),
            (
                "cold_start",
                json!({"iterations": 2, "tool": "echo"}),
                "cold_start",
            ),
            ("race_check", json!({"tool": "echo"}), "race_check"),
            (
                "lost_update",
                json!({
                    "write_tool": "append",
                    "write_args": {"item": "{marker}"},
                    "read_tool": "read"
                }),
                "lost_update",
            ),
            (
                "pattern",
                json!({
                    "duration": "1s",
                    "patterns": [{
                        "name": "read",
                        "steps": [{ "tool": "echo", "args": { "x": 1 } }]
                    }]
                }),
                "pattern",
            ),
            (
                "version_matrix",
                json!({"tool": "echo", "versions": ["2025-03-26", "2025-11-25"]}),
                "version_matrix",
            ),
        ];

        for (kind, params, expected_name) in cases {
            let scenario = build_scenario(kind, &params)
                .unwrap_or_else(|err| panic!("building {kind} failed: {err:#}"));
            assert_eq!(scenario.name(), expected_name);
        }
    }

    #[test]
    fn sustained_accepts_legacy_tool_call_array() {
        let scenario = build_scenario(
            "sustained",
            &json!({
                "duration": "1s",
                "tool_call": [
                    { "name": "echo", "args": { "x": 1 }, "weight": 1.0 },
                    { "name": "echo", "args": { "x": 2 }, "weight": 0.5 }
                ]
            }),
        )
        .expect("legacy tool_call sustained config should build");

        assert_eq!(scenario.name(), "sustained");
    }

    #[test]
    fn example_config_builds_runnable_scenario() {
        let cfg = Config::from_toml_str(&mcp_loadtest::config::example_config())
            .expect("example config should parse");
        let scenario = build_scenario(&cfg.scenario.kind, &cfg.scenario.params)
            .expect("example config scenario should build");

        assert_eq!(scenario.name(), "sustained");
    }

    #[test]
    fn cold_start_requires_tool() {
        // cold_start drives a real first call per fresh session, so `tool`
        // is required (same contract as deadlock_probe).
        let err = match build_scenario("cold_start", &json!({"iterations": 2})) {
            Ok(_) => panic!("cold_start without tool must error"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("tool"), "got: {err:#}");
    }

    #[test]
    fn version_matrix_rejects_unsupported_revision() {
        let err = match build_scenario(
            "version_matrix",
            &json!({"tool": "echo", "versions": ["2019-01-01"]}),
        ) {
            Ok(_) => panic!("unsupported revision must error"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("unsupported revision"),
            "got: {err:#}"
        );
    }

    #[test]
    fn rejects_zero_or_impossible_scenario_bounds() {
        let cases = [
            (
                "sustained",
                json!({"concurrent": 0, "tool": "echo"}),
                "concurrent",
            ),
            (
                "sustained",
                json!({"duration": "0s", "tool": "echo"}),
                "duration",
            ),
            (
                "deadlock_probe",
                json!({"concurrent": 0, "tool": "echo"}),
                "concurrent",
            ),
            (
                "cold_start",
                json!({"iterations": 0, "tool": "echo"}),
                "iterations",
            ),
            (
                "cold_start",
                json!({"iterations": 1, "warmup": true, "tool": "echo"}),
                ">= 2",
            ),
            (
                "race_check",
                json!({"concurrent": 1, "tool": "echo"}),
                "at least 2",
            ),
            (
                "lost_update",
                json!({
                    "write_tool": "append",
                    "write_args": {"item": "fixed"},
                    "read_tool": "read"
                }),
                "{marker}",
            ),
            (
                "lost_update",
                json!({
                    "concurrent": 1,
                    "write_tool": "append",
                    "write_args": {"item": "{marker}"},
                    "read_tool": "read"
                }),
                "concurrent",
            ),
            (
                "pattern",
                json!({
                    "duration": "0s",
                    "patterns": [{"steps": [{"tool": "echo"}]}]
                }),
                "duration",
            ),
            (
                "version_matrix",
                json!({"calls_per_version": 0, "tool": "echo"}),
                "calls_per_version",
            ),
        ];

        for (kind, params, expected) in cases {
            let err = match build_scenario(kind, &params) {
                Ok(_) => panic!("{kind} accepted invalid params {params}"),
                Err(err) => err,
            };
            assert!(
                err.to_string().contains(expected),
                "{kind}: expected {expected:?} in {err:#}"
            );
        }
    }
}
