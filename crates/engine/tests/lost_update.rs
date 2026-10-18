//! `lost_update` against `mock-store.py` in its three modes:
//!
//! - `racy`: an unlocked read-modify-write on a shared file loses writes,
//!   so the run must fail with `lost_update_count > 0`.
//! - `safe`: append-only writes keep every marker, so the run passes.
//! - `memory`: each process keeps its own list, so the read sees only the
//!   control write; the check is inconclusive, not a lost-update finding.

mod helpers;

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mcp_loadtest_core::config::Config;
use mcp_loadtest_core::report::Report;
use mcp_loadtest_engine::Run;
use mcp_loadtest_engine::scenario::lost_update::LostUpdate;
use serde_json::json;

const TEST_TIMEOUT: Duration = Duration::from_secs(90);

/// Unique scratch dir under the OS temp dir, removed on drop.
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p = std::env::temp_dir().join(format!(
            "mcp-loadtest-lost-update-{tag}-{}-{nanos}",
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

async fn run_store(mode: &str, concurrent: u32, writes_per_worker: u32) -> Report {
    let scratch = ScratchDir::new(mode);
    let store = scratch.0.join("store");
    let mock = helpers::fixture_path("mock-store.py");
    let toml = format!(
        r#"
        [server]
        command = '{py}'
        args = ['{mock}', '--mode', '{mode}', '--store', '{store}']
        [scenario]
        type = "lost_update"
        "#,
        py = helpers::python(),
        mock = mock.display(),
        store = store.display(),
    );
    let config = Config::from_toml_str(&toml).expect("config must parse");
    let scenario = Box::new(LostUpdate {
        concurrent,
        writes_per_worker,
        write_tool: "append".to_owned(),
        write_args: json!({ "item": "{marker}" }),
        read_tool: "read".to_owned(),
        read_args: json!({}),
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
async fn racy_read_modify_write_store_loses_updates_and_fails() {
    let report = run_store("racy", 6, 4).await;
    let outcome = &report.scenario_outcome;
    assert!(
        outcome.lost_update_count > 0,
        "6 unlocked writers must lose some of 24 writes: {outcome:?}"
    );
    assert!(!report.passed(), "lost updates must fail the run");
    assert!(
        outcome.notes.iter().any(|n| n.starts_with("lost updates:")),
        "{:?}",
        outcome.notes
    );
}

#[tokio::test]
async fn append_only_store_keeps_every_write_and_passes() {
    let report = run_store("safe", 6, 4).await;
    let outcome = &report.scenario_outcome;
    assert_eq!(outcome.lost_update_count, 0, "{outcome:?}");
    // Control write + read, 24 concurrent writes, final read.
    assert_eq!(outcome.total_calls, 27, "{outcome:?}");
    assert!(report.passed(), "{outcome:?}");
}

#[tokio::test]
async fn per_process_state_is_inconclusive_not_lost_updates() {
    let report = run_store("memory", 3, 2).await;
    let outcome = &report.scenario_outcome;
    assert_eq!(outcome.lost_update_count, 0, "{outcome:?}");
    assert!(!report.passed(), "an inconclusive check must not pass");
    assert!(
        outcome
            .notes
            .iter()
            .any(|n| n.contains("do not appear to share state")),
        "{:?}",
        outcome.notes
    );
}
