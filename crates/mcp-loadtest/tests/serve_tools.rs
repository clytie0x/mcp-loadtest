//! End-to-end checks of the `serve --mcp` tools an agent calls, driven
//! through `serve::tools::dispatch` against the engine's Python mock servers.
//!
//! Each check asserts the verdict contract every run tool shares: a boolean
//! `passed`, a `failure_reasons` list that is empty exactly when `passed`
//! is true, and the tool-specific fields an agent branches on.

use std::path::PathBuf;

use mcp_loadtest::serve::tools::dispatch;
use serde_json::{Value, json};

/// `<python> <fixture>` as the whitespace-split `server_command` the tools
/// take. Override the interpreter with `MCP_LOADTEST_PYTHON`, as the engine
/// tests do.
fn server_command(fixture: &str) -> String {
    let python = std::env::var("MCP_LOADTEST_PYTHON").unwrap_or_else(|_| "python".to_string());
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../engine/tests/fixtures")
        .join(fixture);
    let path = path.display().to_string();
    assert!(
        !path.contains(char::is_whitespace),
        "server_command is whitespace-split; fixture path must not contain spaces: {path}"
    );
    format!("{python} {path}")
}

fn assert_verdict(result: &Value, expect_pass: bool) {
    assert_eq!(result["passed"], expect_pass, "{result:#}");
    let reasons = result["failure_reasons"]
        .as_array()
        .unwrap_or_else(|| panic!("failure_reasons must be an array: {result:#}"));
    assert_eq!(reasons.is_empty(), expect_pass, "{result:#}");
    assert!(result["run_id"].is_string(), "{result:#}");
    assert!(result["notes"].is_array(), "{result:#}");
}

#[tokio::test]
async fn list_target_tools_returns_names_and_schemas() {
    let result = dispatch(
        "list_target_tools",
        &json!({ "server_command": server_command("mock-normal.py") }),
    )
    .await
    .expect("list_target_tools");

    assert_eq!(result["tool_count"], 1, "{result:#}");
    assert_eq!(result["tools"][0]["name"], "echo", "{result:#}");
    assert_eq!(result["tools"][0]["input_schema"]["type"], "object");
    assert!(result["protocol_version"].is_string(), "{result:#}");
}

#[tokio::test]
async fn race_check_passes_a_deterministic_tool() {
    let result = dispatch(
        "race_check",
        &json!({
            "server_command": server_command("mock-normal.py"),
            "tool": "echo",
            "concurrent": 3,
            "args": { "x": 1 },
        }),
    )
    .await
    .expect("race_check");

    assert_verdict(&result, true);
    assert_eq!(result["divergence_count"], 0, "{result:#}");
    assert_eq!(result["successful_calls"], 3, "{result:#}");
}

#[tokio::test]
async fn race_check_fails_with_a_reason_when_responses_diverge() {
    let result = dispatch(
        "race_check",
        &json!({
            "server_command": server_command("mock-process-id.py"),
            "tool": "process_id",
            "concurrent": 3,
        }),
    )
    .await
    .expect("race_check");

    assert_verdict(&result, false);
    assert_eq!(result["divergence_count"], 1, "{result:#}");
    let reasons = result["failure_reasons"].to_string();
    assert!(reasons.contains("divergence"), "{result:#}");
}

#[tokio::test]
async fn cold_start_times_handshake_and_first_call_separately() {
    let result = dispatch(
        "cold_start",
        &json!({
            "server_command": server_command("mock-normal.py"),
            "tool": "echo",
            "iterations": 3,
            "warmup": true,
        }),
    )
    .await
    .expect("cold_start");

    assert_verdict(&result, true);
    // Three iterations, the warm-up excluded from timings.
    assert_eq!(result["handshake"]["count"], 2, "{result:#}");
    assert_eq!(result["first_call"]["count"], 2, "{result:#}");
    assert!(result["first_call"]["max_ms"].as_f64().unwrap_or(-1.0) >= 0.0);
}

#[tokio::test]
async fn run_tools_reject_bad_arguments_before_spawning() {
    for (tool, args) in [
        (
            "race_check",
            json!({ "server_command": "python x.py", "tool": "echo", "concurrent": 1 }),
        ),
        (
            "cold_start",
            json!({ "server_command": "python x.py", "tool": "echo", "iterations": 1 }),
        ),
        (
            "deadlock_probe",
            json!({ "server_command": "python x.py", "tool": "echo", "concurrent": -1 }),
        ),
    ] {
        let err = dispatch(tool, &args)
            .await
            .expect_err("invalid arguments must be rejected");
        assert!(
            err.to_string().starts_with("invalid arguments"),
            "{tool}: {err}"
        );
    }
}
