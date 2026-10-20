//! Lost-update regression test: `LostUpdate` against a real server pinned to
//! a release with the bug.
//!
//! ## Why this test exists
//!
//! `mock-store.py` (covered by `tests/lost_update.rs`) proves the detector
//! against a fixture. This test proves it against real code, as
//! `vibe_trading_regression.rs` does for deadlocks.
//! `@kazuph/mcp-taskmanager` 1.1.1 loads `tasks.json`, changes it and saves
//! it with no lock (docs/field-results.md, run 2026-10-19), so it has both
//! lost-update classes the field runs found:
//!
//! - writes pipelined on one session race inside one process;
//! - writers in separate processes sharing the file overwrite each other,
//!   and drop the control write stored before the run.
//!
//! npm versions are immutable, so pinning `1.1.1` pins the buggy code.
//!
//! ## Running it
//!
//! Marked `#[ignore]` because it needs Node.js with `npx` and network access
//! to the npm registry on first run. Default `cargo test` skips it:
//!
//! ```bash
//! cargo test -p mcp-loadtest-engine --test taskmanager_regression -- --ignored --nocapture
//! ```
//!
//! `MCP_LOADTEST_NPX` overrides the `npx` executable.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mcp_loadtest_core::config::Config;
use mcp_loadtest_core::report::Report;
use mcp_loadtest_engine::Run;
use mcp_loadtest_engine::scenario::lost_update::LostUpdate;
use serde_json::json;

const PACKAGE: &str = "@kazuph/mcp-taskmanager@1.1.1";
const TEST_TIMEOUT: Duration = Duration::from_secs(300);

/// Unique scratch dir under the OS temp dir, removed on drop.
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p = std::env::temp_dir().join(format!(
            "mcp-loadtest-taskmanager-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&p).expect("create scratch dir");
        Self(p)
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn npx() -> String {
    std::env::var("MCP_LOADTEST_NPX")
        .unwrap_or_else(|_| if cfg!(windows) { "npx.cmd" } else { "npx" }.to_owned())
}

async fn run_taskmanager(
    tag: &str,
    concurrent: u32,
    writes_per_worker: u32,
    pipelined: bool,
) -> Report {
    let scratch = ScratchDir::new(tag);
    let tasks_file = scratch.0.join("tasks.json");
    let toml = format!(
        r#"
        [server]
        command = '{npx}'
        args = ['-y', '{PACKAGE}']
        startup_timeout = "120s"
        [server.env]
        TASK_MANAGER_FILE_PATH = '{tasks}'
        [scenario]
        type = "lost_update"
        "#,
        npx = npx(),
        tasks = tasks_file.display(),
    );
    let config = Config::from_toml_str(&toml).expect("config must parse");
    let scenario = Box::new(LostUpdate {
        concurrent,
        writes_per_worker,
        write_tool: "request_planning".to_owned(),
        write_args: json!({
            "originalRequest": "{marker}",
            "tasks": [{ "title": "{marker}", "description": "{marker}" }],
        }),
        read_tool: "list_requests".to_owned(),
        read_args: json!({}),
        pipelined,
        recheck_losses: false,
    });
    tokio::time::timeout(
        TEST_TIMEOUT,
        Run::new(config, scenario, scratch.0.join("runs")).execute(),
    )
    .await
    .expect("run timed out")
    .expect("run should complete")
}

#[tokio::test]
#[ignore = "requires Node.js, npx and the npm registry; run with --ignored"]
async fn pipelined_request_planning_loses_writes_inside_one_process() {
    let report = run_taskmanager("pipelined", 5, 3, true).await;
    let outcome = &report.scenario_outcome;
    // Control write + read, 3 rounds of 5 writes, final read.
    assert_eq!(outcome.total_calls, 18, "{outcome:?}");
    assert_eq!(outcome.error_count, 0, "every call succeeds: {outcome:?}");
    assert!(
        outcome.lost_update_count > 0,
        "{PACKAGE} must lose pipelined writes: {outcome:?}"
    );
    assert!(!report.passed(), "{outcome:?}");
}

#[tokio::test]
#[ignore = "requires Node.js, npx and the npm registry; run with --ignored"]
async fn separate_processes_overwrite_each_other_and_the_control_write() {
    let report = run_taskmanager("processes", 4, 3, false).await;
    let outcome = &report.scenario_outcome;
    assert!(
        outcome.lost_update_count > 0,
        "{PACKAGE} must lose writes across processes: {outcome:?}"
    );
    assert!(!report.passed(), "{outcome:?}");
    assert!(
        outcome
            .notes
            .iter()
            .any(|n| n.contains("control write") && n.contains("already held")),
        "{:?}",
        outcome.notes
    );
}
