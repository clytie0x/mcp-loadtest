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
//! 4. `lost_update`    — wraps [`crate::scenario::lost_update::LostUpdate`].
//! 5. `cold_start`     — wraps [`crate::scenario::cold_start::ColdStart`].
//! 6. `sustained_load` — wraps [`crate::scenario::sustained::Sustained`].
//! 7. `compare_runs`   — reads two `metrics.json` files and diffs them.
//!
//! Every run tool returns the same verdict fields (built by `verdict`) so an
//! agent can branch on `passed` and read `failure_reasons` without knowing
//! which tool produced them. Handlers return `Result<Value, ToolError>`; the
//! caller (in `mod.rs`) maps errors to JSON-RPC error objects.
//!
//! M7 ownership: Agent W. M8 split: per-tool files.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::SpawnOptions;
use crate::config::{self, Config, OutputConfig, ScenarioConfig, ServerConfig};
use crate::report::{Report, failure_reasons};
use crate::run::Run;
use crate::scenario::Scenario;
use crate::session::Session;

mod cold_start;
mod compare_runs;
mod deadlock_probe;
mod list_target_tools;
mod lost_update;
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
    let mut run_tools = vec![
        deadlock_probe::deadlock_probe_def(),
        race_check::race_check_def(),
        lost_update::lost_update_def(),
        cold_start::cold_start_def(),
        sustained_load::sustained_load_def(),
    ];
    for def in &mut run_tools {
        def["inputSchema"]["properties"]["report_dir"] = json!({
            "type": "string",
            "default": DEFAULT_REPORT_DIR,
            "description": "Directory for the run's artifacts (one subdirectory per run). \
                            Relative paths resolve against the server's working directory."
        });
    }
    let mut defs = vec![list_target_tools::list_target_tools_def()];
    defs.extend(run_tools);
    for def in &mut defs {
        add_stdio_target_properties(&mut def["inputSchema"]["properties"]);
    }
    defs.push(compare_runs::compare_runs_def());
    defs
}

/// Route a `tools/call` invocation to the right handler.
pub async fn dispatch(name: &str, arguments: &Value) -> Result<Value, ToolError> {
    match name {
        "list_target_tools" => list_target_tools::list_target_tools(arguments).await,
        "deadlock_probe" => deadlock_probe::deadlock_probe(arguments).await,
        "race_check" => race_check::race_check(arguments).await,
        "lost_update" => lost_update::lost_update(arguments).await,
        "cold_start" => cold_start::cold_start(arguments).await,
        "sustained_load" => sustained_load::sustained_load(arguments).await,
        "compare_runs" => compare_runs::compare_runs(arguments).await,
        other => Err(ToolError::InvalidArgs(format!("unknown tool: {other}"))),
    }
}

// ---- shared helpers -----------------------------------------------------

/// Schema for the optional stdio settings [`stdio_target`] reads.
fn add_stdio_target_properties(properties: &mut Value) {
    properties["env"] = json!({
        "type": "object",
        "additionalProperties": { "type": "string" },
        "description": "Extra environment variables for every server process, merged into \
                        this server's environment (e.g. a data file path or an API key)."
    });
    properties["working_dir"] = json!({
        "type": "string",
        "description": "Working directory for every server process. Defaults to this server's."
    });
    properties["startup_timeout_ms"] = json!({
        "type": "integer",
        "minimum": 1,
        "maximum": MAX_STARTUP_TIMEOUT_MS,
        "default": 10000,
        "description": "Budget for each server's initialize handshake. Raise it for servers \
                        that download or compile on first start (e.g. npx, uvx)."
    });
}

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

/// How to start the target over stdio: the command plus the optional
/// environment, working directory, and startup budget every tool that
/// spawns it accepts.
pub(super) struct StdioTarget {
    command: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    working_dir: Option<PathBuf>,
    startup_timeout: Duration,
}

/// Longest `startup_timeout_ms` an agent may ask for.
const MAX_STARTUP_TIMEOUT_MS: u64 = 600_000;

/// Read `server_command` (required) plus the optional `env`, `working_dir`,
/// and `startup_timeout_ms`.
pub(super) fn stdio_target(args: &Value) -> Result<StdioTarget, ToolError> {
    let (command, cmd_args) = split_server_command(&required_str(args, "server_command")?)?;
    let env = match args.get("env") {
        None => BTreeMap::new(),
        Some(Value::Object(vars)) => vars
            .iter()
            .map(|(key, value)| {
                if key.is_empty() || key.contains(['=', '\0']) {
                    return Err(ToolError::InvalidArgs(format!(
                        "env name {key:?} must be non-empty without '=' or NUL"
                    )));
                }
                match value.as_str() {
                    Some(v) if !v.contains('\0') => Ok((key.clone(), v.to_owned())),
                    _ => Err(ToolError::InvalidArgs(format!(
                        "env value for {key} must be a string without NUL"
                    ))),
                }
            })
            .collect::<Result<_, _>>()?,
        Some(_) => {
            return Err(ToolError::InvalidArgs(
                "env must be an object of string values".into(),
            ));
        }
    };
    let working_dir = match args.get("working_dir") {
        None => None,
        Some(v) => match v.as_str() {
            Some(dir) if !dir.trim().is_empty() => Some(PathBuf::from(dir)),
            _ => {
                return Err(ToolError::InvalidArgs(
                    "working_dir must be a non-empty string".into(),
                ));
            }
        },
    };
    let startup_timeout = match args.get("startup_timeout_ms") {
        None => ServerConfig::stdio(String::new(), Vec::new()).startup_timeout,
        Some(v) => match v.as_u64() {
            Some(ms @ 1..=MAX_STARTUP_TIMEOUT_MS) => Duration::from_millis(ms),
            _ => {
                return Err(ToolError::InvalidArgs(format!(
                    "startup_timeout_ms must be an integer in 1..={MAX_STARTUP_TIMEOUT_MS}"
                )));
            }
        },
    };
    Ok(StdioTarget {
        command,
        args: cmd_args,
        env,
        working_dir,
        startup_timeout,
    })
}

impl StdioTarget {
    fn server_config(&self) -> ServerConfig {
        let mut server = ServerConfig::stdio(self.command.clone(), self.args.clone());
        server.env = self.env.clone();
        server.working_dir = self.working_dir.clone();
        server.startup_timeout = self.startup_timeout;
        server
    }

    /// Spawn one session and run the handshake.
    pub(super) async fn spawn(&self) -> Result<Session, crate::session::SessionError> {
        let opts = SpawnOptions::default()
            .with_env(self.env.clone())
            .with_working_dir(self.working_dir.clone());
        Session::spawn_with_timeout(&self.command, &self.args, opts, self.startup_timeout).await
    }

    pub(super) fn startup_timeout(&self) -> Duration {
        self.startup_timeout
    }
}

/// The server and artifact location every run tool takes.
pub(super) struct RunTarget {
    stdio: StdioTarget,
    report_dir: PathBuf,
}

/// Read the stdio target and `report_dir` (default `./runs`, relative to the
/// server's working directory).
pub(super) fn run_target(args: &Value) -> Result<RunTarget, ToolError> {
    let report_dir = match args.get("report_dir") {
        None => PathBuf::from(DEFAULT_REPORT_DIR),
        Some(v) => match v.as_str() {
            Some(dir) if !dir.trim().is_empty() => PathBuf::from(dir),
            _ => {
                return Err(ToolError::InvalidArgs(
                    "report_dir must be a non-empty string".into(),
                ));
            }
        },
    };
    Ok(RunTarget {
        stdio: stdio_target(args)?,
        report_dir,
    })
}

const DEFAULT_REPORT_DIR: &str = "./runs";

/// Spawn the target over stdio and drive `scenario` to completion. `kind`
/// is the scenario's config `type` string.
pub(super) async fn run_stdio(
    target: &RunTarget,
    kind: &str,
    scenario: Box<dyn Scenario>,
) -> Result<Report, ToolError> {
    // `Config`/`ScenarioConfig`/`OutputConfig` are `#[non_exhaustive]` and
    // live in `mcp-loadtest-core` — build via the constructors + builders
    // rather than an exhaustive struct literal (rejected across the crate
    // boundary).
    let config = Config::new(
        target.stdio.server_config(),
        ScenarioConfig::new(kind, json!({})),
    )
    .with_output(OutputConfig::new(target.report_dir.clone(), vec![]));
    Run::new(config, scenario, target.report_dir.clone())
        .execute()
        .await
        .map_err(|e| ToolError::Run(e.to_string()))
}

/// The verdict fields every run tool returns: `passed`, the reasons it
/// failed (empty when it passed), the gates an agent may want to inspect,
/// the scenario notes, and the `run_id` of the artifacts under `./runs`.
pub(super) fn verdict(report: &Report, target: &RunTarget) -> Map<String, Value> {
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
    out.insert(
        "run_dir".into(),
        json!(target.report_dir.join(&report.run_id).display().to_string()),
    );
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
                "lost_update",
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
