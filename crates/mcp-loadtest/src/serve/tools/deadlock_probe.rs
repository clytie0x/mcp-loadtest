//! `deadlock_probe` tool — handler + JSON schema definition.
//!
//! See [`crate::scenario::deadlock_probe::DeadlockProbe`] for the underlying
//! scenario. Split out of `tools.rs` in M8 to keep per-tool files under the
//! 300-LoC convention.

use std::time::Duration;

use serde_json::{Value, json};

use crate::scenario::deadlock_probe::DeadlockProbe;

use super::{ToolError, required_str, run_stdio, u32_arg, verdict};

pub(super) fn deadlock_probe_def() -> Value {
    json!({
        "name": "deadlock_probe",
        "description":
            "Probe an MCP server for the Vibe-Trading-style deadlock bug class. \
             Spawns the target, fires N tool calls, and classifies each as \
             success / hang / deadlock / error. Returns a fail-closed `passed` \
             signal including worker and teardown completeness.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "server_command": {
                    "type": "string",
                    "description": "Shell-split server command (e.g. \"python -m my_mcp\")."
                },
                "tool": {
                    "type": "string",
                    "description": "Tool name to invoke on the target."
                },
                "concurrent": {
                    "type": "integer",
                    "default": 5,
                    "description": "Number of probe iterations."
                },
                "hang_threshold_ms": {
                    "type": "integer",
                    "default": 2000,
                    "description": "Per-call ms after which a call counts as hung."
                },
                "grace_period_ms": {
                    "type": "integer",
                    "default": 5000,
                    "description": "Extra ms before classifying a hang as a deadlock."
                },
                "args": {
                    "type": "object",
                    "description": "Optional JSON args object passed to every tool call."
                }
            },
            "required": ["server_command", "tool"]
        }
    })
}

pub(super) async fn deadlock_probe(args: &Value) -> Result<Value, ToolError> {
    let server_command = required_str(args, "server_command")?;
    let tool = required_str(args, "tool")?;
    let concurrent = u32_arg(args, "concurrent", 5)?;
    let hang_threshold_ms = args
        .get("hang_threshold_ms")
        .and_then(Value::as_u64)
        .unwrap_or(2_000);
    let grace_period_ms = args
        .get("grace_period_ms")
        .and_then(Value::as_u64)
        .unwrap_or(5_000);
    let scenario = DeadlockProbe {
        concurrent,
        hang_threshold: Duration::from_millis(hang_threshold_ms),
        grace_period: Duration::from_millis(grace_period_ms),
        tool,
        args: args.get("args").cloned().unwrap_or(json!({})),
    };
    let report = run_stdio(&server_command, "deadlock_probe", Box::new(scenario)).await?;

    let mut out = verdict(&report);
    out.insert(
        "deadlock_count".into(),
        json!(report.scenario_outcome.deadlock_count),
    );
    out.insert(
        "hang_count".into(),
        json!(report.scenario_outcome.hang_count),
    );
    // Read the deadlock durations from the typed `ScenarioOutcome` field
    // rather than re-parsing the human-readable notes.
    out.insert(
        "hung_for_ms".into(),
        json!(report.scenario_outcome.hung_for_ms),
    );
    // Kept for callers of the pre-v0.3.0 shape; `notes` is the same list.
    out.insert(
        "scenario_outcome".into(),
        json!({ "notes": report.scenario_outcome.notes }),
    );
    Ok(Value::Object(out))
}
