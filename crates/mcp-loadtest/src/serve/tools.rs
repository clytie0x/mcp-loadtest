//! Tool registry + dispatch for the self-hosted MCP server.
//!
//! Each tool lives in its own submodule and exposes a `*_def()` constructor +
//! a handler async fn via `pub(super)`. This file keeps the small public
//! surface — `ToolError`, `ToolDef`, `tool_defs`, `dispatch` — plus the
//! helpers shared across tools.
//!
//! See DESIGN.md §21.2 for the motivation. The tools, in the order an agent
//! typically calls them:
//!
//! 1. `list_target_tools` — the target's `tools/list`, so the agent knows what
//!    to probe.
//! 2. `deadlock_probe` — wraps [`crate::scenario::deadlock_probe::DeadlockProbe`].
//! 3. `race_check`     — wraps [`crate::scenario::race_check::RaceCheck`].
//! 4. `cold_start`     — wraps [`crate::scenario::cold_start::ColdStart`].
//! 5. `sustained_load` — wraps [`crate::scenario::sustained::Sustained`].
//! 6. `compare_runs`   — reads two `metrics.json` files and diffs them.
//!
//! Every run tool returns the same verdict fields (built by `verdict`) so an
//! agent can branch on `passed` and read `failure_reasons` without knowing
//! which tool produced them. Handlers return `Result<Value, ToolError>`; the
//! caller (in `mod.rs`) maps errors to JSON-RPC error objects.
//!
//! M7 ownership: Agent W. M8 split: per-tool files.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::config::{self, Config, OutputConfig, ScenarioConfig, ServerConfig};
use crate::report::{Report, failure_reasons};
use crate::run::Run;
use crate::scenario::Scenario;

mod cold_start;
mod compare_runs;
mod deadlock_probe;
mod list_target_tools;
mod race_check;
mod sustained_load;

/// Errors a tool handler can produce. The server transport translates these
/// into JSON-RPC error objects.
#[derive(Debug, Error)]
pub enum ToolError {
    /// Caller passed bad arguments (missing field, wrong type, ...).
    #[error("invalid arguments: {0}")]
    InvalidArgs(String),
    /// The underlying run failed.
    #[error("run failed: {0}")]
    Run(String),
    /// I/O error reading a metrics.json file in compare_runs.
    #[error("io: {0}")]
    Io(String),
}

/// Public description of a tool — what we surface in `tools/list`.
pub struct ToolDef {
    /// Tool name (called via `tools/call`).
    pub name: &'static str,
    /// One-line summary used as the MCP `description`.
    pub description: &'static str,
    /// JSON-schema-flavored input description for the tool's args.
    pub input_schema: Value,
}

/// Return the full registry as plain JSON values (the shape `tools/list`
/// expects under `result.tools`).
pub fn tool_defs() -> Vec<Value> {
    vec![
        list_target_tools::list_target_tools_def(),
        deadlock_probe::deadlock_probe_def(),
        race_check::race_check_def(),
        cold_start::cold_start_def(),
        sustained_load::sustained_load_def(),
        compare_runs::compare_runs_def(),
    ]
}

/// Route a `tools/call` invocation to the right handler.
pub async fn dispatch(name: &str, arguments: &Value) -> Result<Value, ToolError> {
    match name {
        "list_target_tools" => list_target_tools::list_target_tools(arguments).await,
        "deadlock_probe" => deadlock_probe::deadlock_probe(arguments).await,
        "race_check" => race_check::race_check(arguments).await,
        "cold_start" => cold_start::cold_start(arguments).await,
        "sustained_load" => sustained_load::sustained_load(arguments).await,
        "compare_runs" => compare_runs::compare_runs(arguments).await,
        other => Err(ToolError::InvalidArgs(format!("unknown tool: {other}"))),
    }
}

// ---- shared helpers -----------------------------------------------------

pub(super) fn required_str(args: &Value, key: &str) -> Result<String, ToolError> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ToolError::InvalidArgs(format!("{key} (string) is required")))
}

/// Read an optional integer argument that must fit in `u32`, falling back to
/// `default` when absent.
pub(super) fn u32_arg(args: &Value, key: &str, default: u32) -> Result<u32, ToolError> {
    match args.get(key) {
        None => Ok(default),
        Some(v) => v
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| {
                ToolError::InvalidArgs(format!("{key} must be an integer in 0..=u32::MAX"))
            }),
    }
}

/// Spawn `server_command` over stdio and drive `scenario` to completion.
/// `kind` is the scenario's config `type` string.
pub(super) async fn run_stdio(
    server_command: &str,
    kind: &str,
    scenario: Box<dyn Scenario>,
) -> Result<Report, ToolError> {
    let (command, cmd_args) = split_server_command(server_command)?;
    // `Config`/`ScenarioConfig`/`OutputConfig` are `#[non_exhaustive]` and
    // live in `mcp-loadtest-core` — build via the constructors + builders
    // rather than an exhaustive struct literal (rejected across the crate
    // boundary).
    let config = Config::new(
        ServerConfig::stdio(command, cmd_args),
        ScenarioConfig::new(kind, json!({})),
    )
    .with_output(OutputConfig::new(PathBuf::from("./runs"), vec![]));
    Run::new(config, scenario, PathBuf::from("./runs"))
        .execute()
        .await
        .map_err(|e| ToolError::Run(e.to_string()))
}

/// The verdict fields every run tool returns: `passed`, the reasons it
/// failed (empty when it passed), the gates an agent may want to inspect,
/// the scenario notes, and the `run_id` of the artifacts under `./runs`.
pub(super) fn verdict(report: &Report) -> Map<String, Value> {
    let passed = report.passed();
    let reasons = if passed {
        Vec::new()
    } else {
        failure_reasons(report)
    };
    let mut out = Map::new();
    out.insert("passed".into(), json!(passed));
    out.insert("failure_reasons".into(), json!(reasons));
    out.insert(
        "total_calls".into(),
        json!(report.scenario_outcome.total_calls),
    );
    out.insert(
        "successful_calls".into(),
        json!(report.scenario_outcome.successful_calls),
    );
    out.insert(
        "error_count".into(),
        json!(report.scenario_outcome.error_count),
    );
    out.insert(
        "incomplete_worker_count".into(),
        json!(report.scenario_outcome.incomplete_worker_count),
    );
    out.insert(
        "teardown_failure_count".into(),
        json!(report.scenario_outcome.teardown_failure_count),
    );
    out.insert(
        "threshold_violation_count".into(),
        json!(report.threshold_violations.len()),
    );
    out.insert("notes".into(), json!(report.scenario_outcome.notes));
    out.insert("run_id".into(), json!(report.run_id));
    out
}

pub(super) fn duration_ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Thin adapter around [`config::split_server_command`] that translates the
/// `ConfigError` into a `ToolError::InvalidArgs` so the JSON-RPC error mapping
/// stays right.
pub(super) fn split_server_command(s: &str) -> Result<(String, Vec<String>), ToolError> {
    config::split_server_command(s).map_err(|e| ToolError::InvalidArgs(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_defs_list_every_dispatchable_tool_with_a_schema() {
        let defs = tool_defs();
        let names: Vec<&str> = defs
            .iter()
            .filter_map(|d| d.get("name").and_then(Value::as_str))
            .collect();
        assert_eq!(
            names,
            [
                "list_target_tools",
                "deadlock_probe",
                "race_check",
                "cold_start",
                "sustained_load",
                "compare_runs",
            ]
        );
        for def in &defs {
            assert_eq!(def["inputSchema"]["type"], "object", "{def}");
            assert!(def["description"].is_string(), "{def}");
        }
    }

    #[test]
    fn u32_arg_defaults_and_rejects_out_of_range() {
        assert_eq!(u32_arg(&json!({}), "n", 7).unwrap(), 7);
        assert_eq!(u32_arg(&json!({"n": 3}), "n", 7).unwrap(), 3);
        assert!(u32_arg(&json!({"n": -1}), "n", 7).is_err());
        assert!(u32_arg(&json!({"n": u64::from(u32::MAX) + 1}), "n", 7).is_err());
        assert!(u32_arg(&json!({"n": "3"}), "n", 7).is_err());
    }

    #[test]
    fn split_server_command_parses_python_invocation() {
        let (cmd, args) = split_server_command("python -m my_mcp --foo bar").unwrap();
        assert_eq!(cmd, "python");
        assert_eq!(args, vec!["-m", "my_mcp", "--foo", "bar"]);
    }

    #[test]
    fn split_server_command_rejects_empty() {
        assert!(split_server_command("   ").is_err());
    }

    #[test]
    fn required_str_returns_invalid_args_when_missing() {
        let v = json!({});
        let err = required_str(&v, "tool").unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgs(_)));
    }

    #[tokio::test]
    async fn dispatch_unknown_tool_returns_invalid_args() {
        let err = dispatch("not_a_tool", &json!({})).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgs(_)));
    }
}
