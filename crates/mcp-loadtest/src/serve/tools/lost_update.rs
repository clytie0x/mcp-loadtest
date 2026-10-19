//! `lost_update` tool — handler + JSON schema definition.
//!
//! See [`crate::scenario::lost_update::LostUpdate`] for the underlying
//! scenario: concurrent uniquely-marked writes across independent sessions,
//! then one read that must contain every acknowledged write.

use serde_json::{Value, json};

use crate::scenario::lost_update::LostUpdate;

use super::{ToolError, required_str, run_scenario, run_target, u32_arg, verdict};

pub(super) fn lost_update_def() -> Value {
    json!({
        "name": "lost_update",
        "description":
            "Detect writes that are acknowledged but lost under concurrency. N \
             independent sessions each send writes_per_worker calls to write_tool, \
             each carrying a unique marker in place of {marker}; then one read_tool \
             call must return every acknowledged marker. Missing markers fail the \
             run (lost_update_count). Writes the server refuses with isError were \
             never acknowledged; they are counted (refused_write_count) but do not \
             fail the run. Use list_target_tools first to pick a write tool and \
             a read tool that returns what it stored. If the sessions do not \
             share state, the result is inconclusive, not a finding.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "server_command": {
                    "type": "string",
                    "description": "Shell-split server command (e.g. \"python -m my_mcp\")."
                },
                "write_tool": {
                    "type": "string",
                    "description": "Tool that records one write."
                },
                "write_args": {
                    "type": "object",
                    "description": "Arguments for write_tool. A string value must contain {marker}; {worker} and {seq} are also substituted."
                },
                "read_tool": {
                    "type": "string",
                    "description": "Tool whose result must contain every acknowledged marker."
                },
                "read_args": {
                    "type": "object",
                    "description": "Optional arguments for read_tool."
                },
                "concurrent": {
                    "type": "integer",
                    "minimum": 2,
                    "default": 10,
                    "description": "Independent sessions writing at once (with server_command, each is its own server process)."
                },
                "writes_per_worker": {
                    "type": "integer",
                    "minimum": 1,
                    "default": 5,
                    "description": "Writes each session sends (with pipelined: the number of rounds)."
                },
                "pipelined": {
                    "type": "boolean",
                    "default": false,
                    "description": "Use one session (one server process) and send each round of `concurrent` writes back to back, as a client running tool calls in parallel does. Needs server_command."
                }
            },
            "required": ["server_command", "write_tool", "write_args", "read_tool"]
        }
    })
}

pub(super) async fn lost_update(args: &Value) -> Result<Value, ToolError> {
    let target = run_target(args)?;
    let plan = LostUpdate {
        concurrent: u32_arg(args, "concurrent", 10)?,
        writes_per_worker: u32_arg(args, "writes_per_worker", 5)?,
        write_tool: required_str(args, "write_tool")?,
        write_args: args
            .get("write_args")
            .filter(|v| v.is_object())
            .cloned()
            .ok_or_else(|| ToolError::InvalidArgs("write_args (object) is required".into()))?,
        read_tool: required_str(args, "read_tool")?,
        read_args: args.get("read_args").cloned().unwrap_or(json!({})),
        pipelined: match args.get("pipelined") {
            None => false,
            Some(v) => v
                .as_bool()
                .ok_or_else(|| ToolError::InvalidArgs("pipelined must be a boolean".into()))?,
        },
    };
    if plan.pipelined && args.get("server_url").is_some() {
        return Err(ToolError::InvalidArgs(
            "pipelined needs server_command: it sends requests back to back on one stdio stream"
                .into(),
        ));
    }
    if let Some(message) = plan.validation_error() {
        return Err(ToolError::InvalidArgs(message.into()));
    }
    let report = run_scenario(&target, "lost_update", Box::new(plan)).await?;

    let mut out = verdict(&report, &target);
    out.insert(
        "lost_update_count".into(),
        json!(report.scenario_outcome.lost_update_count),
    );
    out.insert(
        "refused_write_count".into(),
        json!(report.scenario_outcome.refused_write_count),
    );
    Ok(Value::Object(out))
}
