//! `cold_start` tool — handler + JSON schema definition.
//!
//! See [`crate::scenario::cold_start::ColdStart`] for the underlying
//! scenario: a fresh server per iteration, timing the handshake and the first
//! tool call separately. Agents use it to tell a slow start from a hang
//! before choosing `hang_threshold_ms` for `deadlock_probe`.

use serde_json::{Value, json};

use crate::metrics::ScenarioMetrics;
use crate::scenario::cold_start::{ColdStart, HANDSHAKE_METRIC};

use super::{ToolError, duration_ms, required_str, run_stdio, run_target, u32_arg, verdict};

pub(super) fn cold_start_def() -> Value {
    json!({
        "name": "cold_start",
        "description":
            "Measure start-up cost. Spawns a fresh server process per iteration and \
             times the spawn + initialize handshake and the first tool call \
             separately. Use the first-call max to set a deadlock_probe \
             hang_threshold_ms that will not mistake a slow start for a hang.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "server_command": {
                    "type": "string",
                    "description": "Shell-split server command (e.g. \"python -m my_mcp\")."
                },
                "tool": {
                    "type": "string",
                    "description": "Tool called once per fresh server."
                },
                "iterations": {
                    "type": "integer",
                    "minimum": 1,
                    "default": 5,
                    "description": "Fresh servers to start."
                },
                "warmup": {
                    "type": "boolean",
                    "default": true,
                    "description": "Run one extra iteration first and exclude it from the timings (needs iterations >= 2)."
                },
                "args": {
                    "type": "object",
                    "description": "Optional JSON args object passed to the first call."
                }
            },
            "required": ["server_command", "tool"]
        }
    })
}

pub(super) async fn cold_start(args: &Value) -> Result<Value, ToolError> {
    let target = run_target(args)?;
    let tool = required_str(args, "tool")?;
    let iterations = u32_arg(args, "iterations", 5)?;
    let warmup = match args.get("warmup") {
        None => true,
        Some(v) => v
            .as_bool()
            .ok_or_else(|| ToolError::InvalidArgs("warmup must be a boolean".into()))?,
    };
    if iterations == 0 || (warmup && iterations < 2) {
        return Err(ToolError::InvalidArgs(
            "iterations must be >= 1, and >= 2 when warmup is true".into(),
        ));
    }
    let scenario = ColdStart {
        iterations,
        warmup,
        tool: tool.clone(),
        args: args.get("args").cloned().unwrap_or(json!({})),
    };
    let report = run_stdio(&target, "cold_start", Box::new(scenario)).await?;

    let mut out = verdict(&report, &target);
    out.insert(
        "handshake".into(),
        latency_summary(report.per_tool.get(HANDSHAKE_METRIC)),
    );
    out.insert(
        "first_call".into(),
        latency_summary(report.per_tool.get(&tool)),
    );
    Ok(Value::Object(out))
}

/// Count and latency percentiles for one per-tool metric, or `null` when
/// nothing was recorded under it.
fn latency_summary(metrics: Option<&ScenarioMetrics>) -> Value {
    let Some(m) = metrics else {
        return Value::Null;
    };
    json!({
        "count": m.latency.count,
        "p50_ms": duration_ms(m.latency.p50),
        "p95_ms": duration_ms(m.latency.p95),
        "max_ms": duration_ms(m.latency.max),
    })
}
