//! Lost-update regression test: `LostUpdate` against
//! `@modelcontextprotocol/server-filesystem` pinned to a release with the bug.
//!
//! ## Why this test exists
//!
//! `edit_file` reads the file, applies the edits and writes it back with no
//! lock (docs/field-results.md, run 2026-10-19). Since 1.0.0 it also writes
//! in place (`truncate` then write), so a reader in another process can see
//! the file empty or half-written. `mock-store.py --mode inplace` covers that
//! pattern with a fixture; this test pins it to the real server, as
//! `taskmanager_regression.rs` does for a JSON store.
//!
//! Each write replaces `END` with `{marker}\nEND` in one file, and
//! `read_text_file` reads it back.
//!
//! npm versions are immutable, so pinning `2.0.0` pins the buggy code.
//!
//! ## Running it
//!
//! Marked `#[ignore]` because it needs Node.js with `npx` and network access
//! to the npm registry on first run:
//!
//! ```bash
//! cargo test -p mcp-loadtest-engine --test filesystem_regression -- --ignored --nocapture
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

const PACKAGE: &str = "@modelcontextprotocol/server-filesystem@2.0.0";
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
            "mcp-loadtest-filesystem-{tag}-{}-{nanos}",
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

async fn run_filesystem(
    tag: &str,
    concurrent: u32,
    writes_per_worker: u32,
    pipelined: bool,
) -> Report {
    let scratch = ScratchDir::new(tag);
    let root = scratch.0.join("root");
    std::fs::create_dir_all(&root).expect("create allowed dir");
    let notes = root.join("notes.txt");
    std::fs::write(&notes, "END\n").expect("seed notes.txt");
    let toml = format!(
        r#"
        [server]
        command = '{npx}'
        args = ['-y', '{PACKAGE}', '{root}']
        startup_timeout = "120s"
        [scenario]
        type = "lost_update"
        "#,
        npx = npx(),
        root = root.display(),
    );
    let config = Config::from_toml_str(&toml).expect("config must parse");
    let path = notes.display().to_string();
    let scenario = Box::new(LostUpdate {
        concurrent,
        writes_per_worker,
        write_tool: "edit_file".to_owned(),
        write_args: json!({
            "path": path,
            "edits": [{ "oldText": "END", "newText": "{marker}\nEND" }],
        }),
        read_tool: "read_text_file".to_owned(),
        read_args: json!({ "path": path }),
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
async fn pipelined_edit_file_loses_edits_inside_one_process() {
    let report = run_filesystem("pipelined", 5, 3, true).await;
    let outcome = &report.scenario_outcome;
    // Control write + read, 3 rounds of 5 writes, final read.
    assert_eq!(outcome.total_calls, 18, "{outcome:?}");
    assert_eq!(outcome.error_count, 0, "every call succeeds: {outcome:?}");
    assert!(
        outcome.lost_update_count > 0,
        "{PACKAGE} must lose pipelined edits: {outcome:?}"
    );
    assert!(!report.passed(), "{outcome:?}");
}

#[tokio::test]
#[ignore = "requires Node.js, npx and the npm registry; run with --ignored"]
async fn separate_processes_lose_edits_to_one_file() {
    let report = run_filesystem("processes", 6, 4, false).await;
    let outcome = &report.scenario_outcome;
    assert!(
        outcome.lost_update_count > 0,
        "{PACKAGE} must lose edits across processes: {outcome:?}"
    );
    assert!(!report.passed(), "{outcome:?}");
}
