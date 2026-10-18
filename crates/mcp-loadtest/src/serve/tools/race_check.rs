//! `race_check` tool — handler + JSON schema definition.
//!
//! See [`crate::scenario::race_check::RaceCheck`] for the underlying
//! scenario: N independent sessions each send one identical call, released
//! through a shared gate, and the canonicalized responses are compared.

use serde_json::{Value, json};

use crate::scenario::race_check::RaceCheck;

use super::{ToolError, required_str, run_stdio, run_target, u32_arg, verdict};

pub(super) fn race_check_def() -> Value {
    json!({
        "name": "race_check",
        "description":
            "Detect non-deterministic responses. Spawns N independent sessions of \
             the target, releases one identical tool call from each at the same \
             instant, and compares the responses. Any divergence fails the run; \
             `notes` describes the distinct responses. Only use it on tools that \
             should answer identical inputs identically.",
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
                    "minimum": 2,
                    "default": 10,
                    "description": "Number of synchronized sessions (each its own server process)."
                },
                "args": {
                    "type": "object",
                    "description": "Optional JSON args object passed to every call."
                }
            },
            "required": ["server_command", "tool"]
        }
    })
}

pub(super) async fn race_check(args: &Value) -> Result<Value, ToolError> {
    let target = run_target(args)?;
    let tool = required_str(args, "tool")?;
    let concurrent = u32_arg(args, "concurrent", 10)?;
    if concurrent < 2 {
        return Err(ToolError::InvalidArgs(
            "concurrent must be at least 2 to compare responses".into(),
        ));
    }
    let scenario = RaceCheck {
        concurrent,
        tool,
        args: args.get("args").cloned().unwrap_or(json!({})),
    };
    let report = run_stdio(&target, "race_check", Box::new(scenario)).await?;

    let mut out = verdict(&report, &target);
    out.insert(
        "divergence_count".into(),
        json!(report.scenario_outcome.divergence_count),
    );
    out.insert(
        "hang_count".into(),
        json!(report.scenario_outcome.hang_count),
    );
    Ok(Value::Object(out))
}
