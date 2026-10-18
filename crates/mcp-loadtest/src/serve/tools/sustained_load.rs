//! `sustained_load` tool — handler + JSON schema definition.
//!
//! See [`crate::scenario::sustained::Sustained`] for the underlying scenario.
//! Split out of `tools.rs` in M8 to keep per-tool files under the 300-LoC
//! convention.

use std::time::Duration;

use serde_json::{Value, json};

use crate::scenario::sustained::Sustained;

use super::{ToolError, duration_ms, required_str, run_scenario, run_target, u32_arg, verdict};

pub(super) fn sustained_load_def() -> Value {
    json!({
        "name": "sustained_load",
        "description":
            "Run a sustained constant-load workload against an MCP server for the \
             given duration. Returns latency, error rate, throughput, and a \
             fail-closed `passed` signal.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "server_command": {
                    "type": "string",
                    "description": "Shell-split server command."
                },
                "tool": {
                    "type": "string",
                    "description": "Tool name to invoke on every iteration."
                },
                "concurrent": {
                    "type": "integer",
                    "default": 10,
                    "description": "Independent session workers."
                },
                "duration_ms": {
                    "type": "integer",
                    "description": "Total run duration, milliseconds."
                },
                "args": {
                    "type": "object",
                    "description": "Optional JSON args object passed to every tool call."
                }
            },
            "required": ["server_command", "tool", "duration_ms"]
        }
    })
}

pub(super) async fn sustained_load(args: &Value) -> Result<Value, ToolError> {
    let target = run_target(args)?;
    let tool = required_str(args, "tool")?;
    let concurrent = u32_arg(args, "concurrent", 10)?;
    let run_ms = args
        .get("duration_ms")
        .and_then(Value::as_u64)
        .ok_or_else(|| ToolError::InvalidArgs("duration_ms is required".into()))?;
    let scenario = Sustained {
        concurrent,
        duration: Duration::from_millis(run_ms),
        tool,
        args: args.get("args").cloned().unwrap_or(json!({})),
    };
    let report = run_scenario(&target, "sustained", Box::new(scenario)).await?;

    let total = report.metrics.throughput.total_requests;
    let success = report.metrics.throughput.successful_requests;
    let errors = total.saturating_sub(success);
    let error_rate = if total == 0 {
        0.0
    } else {
        errors as f64 / total as f64
    };

    let mut out = verdict(&report, &target);
    out.insert(
        "p50_ms".into(),
        json!(duration_ms(report.metrics.latency.p50)),
    );
    out.insert(
        "p95_ms".into(),
        json!(duration_ms(report.metrics.latency.p95)),
    );
    out.insert(
        "p99_ms".into(),
        json!(duration_ms(report.metrics.latency.p99)),
    );
    out.insert(
        "p999_ms".into(),
        json!(duration_ms(report.metrics.latency.p999)),
    );
    out.insert("error_rate".into(), json!(error_rate));
    out.insert(
        "requests_per_sec".into(),
        json!(report.metrics.throughput.requests_per_sec),
    );
    out.insert("total_requests".into(), json!(total));
    out.insert("successful_requests".into(), json!(success));
    Ok(Value::Object(out))
}
