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

/// A scratch directory under the OS temp dir, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "mcp-loadtest-serve-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn lost_update_reports_missing_writes_with_a_reason() {
    let scratch = Scratch::new("lost-update");
    let store = scratch.0.join("store");
    let result = dispatch(
        "lost_update",
        &json!({
            "server_command": format!(
                "{} --mode racy --store {}",
                server_command("mock-store.py"),
                store.display()
            ),
            "write_tool": "append",
            "write_args": { "item": "{marker}" },
            "read_tool": "read",
            "concurrent": 4,
            "writes_per_worker": 3,
            "report_dir": scratch.0.join("runs").display().to_string(),
        }),
    )
    .await
    .expect("lost_update");

    assert_verdict(&result, false);
    assert!(
        result["lost_update_count"].as_u64().unwrap_or(0) > 0,
        "{result:#}"
    );
    assert!(
        result["failure_reasons"]
            .to_string()
            .contains("lost update"),
        "{result:#}"
    );
    let run_dir = PathBuf::from(result["run_dir"].as_str().expect("run_dir"));
    assert!(run_dir.starts_with(scratch.0.join("runs")), "{result:#}");
    assert!(
        run_dir.is_dir(),
        "run artifacts must land in report_dir: {result:#}"
    );
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
        (
            "lost_update",
            json!({
                "server_command": "python x.py",
                "write_tool": "append",
                "write_args": { "item": "no placeholder" },
                "read_tool": "read",
            }),
        ),
        (
            "race_check",
            json!({ "server_command": "python x.py", "tool": "echo", "report_dir": "" }),
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
