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

/// Initialize budget for every fixture spawn. Cold Python starts on a loaded
/// Windows runner have exceeded the 10 s default.
const STARTUP_TIMEOUT_MS: u64 = 60_000;

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
        &json!({
            "server_command": server_command("mock-normal.py"),
            "startup_timeout_ms": STARTUP_TIMEOUT_MS,
        }),
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
            "startup_timeout_ms": STARTUP_TIMEOUT_MS,
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
            "startup_timeout_ms": STARTUP_TIMEOUT_MS,
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
            "startup_timeout_ms": STARTUP_TIMEOUT_MS,
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
            "startup_timeout_ms": STARTUP_TIMEOUT_MS,
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
async fn env_and_working_dir_reach_every_server_process() {
    let scratch = Scratch::new("target-env");
    // A relative store path only lands in the scratch directory if both
    // `env` and `working_dir` reached each spawned server.
    let result = dispatch(
        "lost_update",
        &json!({
            "server_command": format!("{} --mode safe", server_command("mock-store.py")),
            "env": { "MOCK_STORE_PATH": "store" },
            "working_dir": scratch.0.display().to_string(),
            "startup_timeout_ms": STARTUP_TIMEOUT_MS,
            "write_tool": "append",
            "write_args": { "item": "{marker}" },
            "read_tool": "read",
            "concurrent": 3,
            "writes_per_worker": 2,
            "report_dir": scratch.0.join("runs").display().to_string(),
        }),
    )
    .await
    .expect("lost_update");

    assert_verdict(&result, true);
    assert_eq!(result["lost_update_count"], 0, "{result:#}");
    let stored = std::fs::read_dir(scratch.0.join("store"))
        .expect("store directory created through env + working_dir")
        .count();
    // Six concurrent writes plus the control write.
    assert_eq!(stored, 7, "{result:#}");

    let listed = dispatch(
        "list_target_tools",
        &json!({
            "server_command": format!("{} --mode safe", server_command("mock-store.py")),
            "env": { "MOCK_STORE_PATH": "store" },
            "working_dir": scratch.0.display().to_string(),
            "startup_timeout_ms": STARTUP_TIMEOUT_MS,
        }),
    )
    .await
    .expect("list_target_tools");
    assert_eq!(listed["tool_count"], 2, "{listed:#}");
}

#[test]
fn every_target_tool_advertises_both_target_kinds() {
    for def in mcp_loadtest::serve::tools::tool_defs() {
        let schema = &def["inputSchema"];
        let properties = &schema["properties"];
        let takes_target = properties.get("server_command").is_some();
        for key in [
            "server_url",
            "transport",
            "allowed_hosts",
            "headers_from_env",
            "env",
            "working_dir",
            "startup_timeout_ms",
        ] {
            assert_eq!(
                properties.get(key).is_some(),
                takes_target,
                "{}: {key}",
                def["name"]
            );
        }
        let required = schema["required"].to_string();
        assert!(!required.contains("server_command"), "{}", def["name"]);
    }
}

/// The stdlib streamable-HTTP mock on an OS-assigned loopback port, killed
/// on drop.
struct HttpMock {
    child: std::process::Child,
    url: String,
}

impl HttpMock {
    fn start() -> Self {
        Self::start_with(&[])
    }

    fn start_with(extra: &[&str]) -> Self {
        use std::io::{BufRead, BufReader};
        let python = std::env::var("MCP_LOADTEST_PYTHON").unwrap_or_else(|_| "python".to_string());
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../engine/tests/fixtures/mock-http-server.py");
        let mut child = std::process::Command::new(python)
            .arg(script)
            .args(["--port", "0"])
            .args(extra)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn mock-http-server.py");
        let stdout = child.stdout.take().expect("mock stdout");
        let mut line = String::new();
        BufReader::new(stdout)
            .read_line(&mut line)
            .expect("read LISTENING line");
        let addr = line
            .trim()
            .strip_prefix("LISTENING: ")
            .unwrap_or_else(|| panic!("unexpected mock output: {line:?}"))
            .to_owned();
        Self {
            child,
            url: format!("http://{addr}/"),
        }
    }
}

impl Drop for HttpMock {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test]
async fn remote_targets_list_tools_and_run_detectors() {
    let mock = HttpMock::start();
    let scratch = Scratch::new("remote");

    let listed = dispatch(
        "list_target_tools",
        &json!({ "server_url": mock.url, "allowed_hosts": ["127.0.0.1"] }),
    )
    .await
    .expect("list_target_tools");
    assert_eq!(listed["tool_count"], 1, "{listed:#}");
    assert_eq!(listed["tools"][0]["name"], "echo", "{listed:#}");

    let raced = dispatch(
        "race_check",
        &json!({
            "server_url": mock.url,
            "allowed_hosts": ["127.0.0.1"],
            "tool": "echo",
            "concurrent": 4,
            "args": { "x": 1 },
            "report_dir": scratch.0.join("runs").display().to_string(),
        }),
    )
    .await
    .expect("race_check");
    assert_verdict(&raced, true);
    assert_eq!(raced["successful_calls"], 4, "{raced:#}");
}

#[tokio::test]
async fn lost_update_over_http_separates_racy_from_safe_stores() {
    let scratch = Scratch::new("remote-lost-update");
    let mut lost = Vec::new();
    for mode in ["racy", "safe"] {
        let mock = HttpMock::start_with(&["--store", mode]);
        let result = dispatch(
            "lost_update",
            &json!({
                "server_url": mock.url,
                "allowed_hosts": ["127.0.0.1"],
                "write_tool": "append",
                "write_args": { "item": "{marker}" },
                "read_tool": "read",
                "concurrent": 6,
                "writes_per_worker": 4,
                "report_dir": scratch.0.join(mode).display().to_string(),
            }),
        )
        .await
        .expect("lost_update");
        assert_verdict(&result, mode == "safe");
        lost.push(result["lost_update_count"].as_u64().unwrap_or(u64::MAX));
    }
    assert!(lost[0] > 0, "racy store must lose writes: {lost:?}");
    assert_eq!(lost[1], 0, "safe store must keep every write: {lost:?}");
}

#[tokio::test]
async fn loopback_url_is_blocked_unless_allowed() {
    let mock = HttpMock::start();
    let err = dispatch("list_target_tools", &json!({ "server_url": mock.url }))
        .await
        .expect_err("loopback must be blocked without allowed_hosts");
    assert!(err.to_string().contains("blocked host"), "{err}");
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
        (
            "race_check",
            json!({ "server_command": "python x.py", "tool": "echo", "env": ["A=1"] }),
        ),
        (
            "list_target_tools",
            json!({ "server_command": "python x.py", "env": { "A": 1 } }),
        ),
        (
            "cold_start",
            json!({ "server_command": "python x.py", "tool": "echo", "env": { "A=B": "1" } }),
        ),
        (
            "deadlock_probe",
            json!({ "server_command": "python x.py", "tool": "echo", "working_dir": " " }),
        ),
        (
            "list_target_tools",
            json!({ "server_command": "python x.py", "startup_timeout_ms": 0 }),
        ),
        (
            "list_target_tools",
            json!({ "server_command": "python x.py", "server_url": "https://example.com/mcp" }),
        ),
        ("list_target_tools", json!({})),
        (
            "race_check",
            json!({ "server_url": "https://example.com/mcp", "tool": "echo", "env": { "A": "1" } }),
        ),
        (
            "race_check",
            json!({ "server_url": "https://example.com/mcp", "tool": "echo", "transport": "ws" }),
        ),
        (
            "list_target_tools",
            json!({ "server_url": "https://example.com/mcp", "allowed_hosts": ["https://example.com"] }),
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
