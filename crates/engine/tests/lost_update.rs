//! `lost_update` against `mock-store.py` in its modes:
//!
//! - `racy`: an unlocked read-modify-write on a shared file loses writes,
//!   so the run must fail with `lost_update_count > 0`.
//! - `safe`: one atomically published file per write keeps every marker,
//!   so the run passes.
//! - `last`: each write replaces the store, so the control write and every
//!   concurrent write but the last are lost, deterministically.
//! - `memory`: each process keeps its own list, so the run session never
//!   sees the control written on another session; the check is
//!   inconclusive, not a lost-update finding.
//! - `refusing`: refuses odd-numbered writes with `isError`, as a store with
//!   optimistic concurrency refuses conflicting ones. Every acknowledged
//!   write lands, so the run passes and counts the refusals.
//! - `closed`: refuses every write, the control included; the check cannot
//!   run, and the note carries the server's error text.

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
            .any(|n| n.contains("do not share state")),
        "{:?}",
        outcome.notes
    );
}

#[tokio::test]
async fn a_store_that_drops_earlier_data_also_loses_the_control_write() {
    let report = run_store("last", 4, 3).await;
    let outcome = &report.scenario_outcome;
    // 12 concurrent writes, one survives; the control write is lost too.
    assert_eq!(outcome.lost_update_count, 12, "{outcome:?}");
    assert!(!report.passed(), "{outcome:?}");
    assert!(
        outcome
            .notes
            .iter()
            .any(|n| n.contains("control write") && n.contains("already held")),
        "{:?}",
        outcome.notes
    );
    assert!(
        outcome
            .notes
            .iter()
            .any(|n| n.contains("11 of 12 acknowledged writes")),
        "{:?}",
        outcome.notes
    );
}

#[tokio::test]
async fn refused_writes_are_counted_and_do_not_fail_the_check() {
    let report = run_store("refusing", 3, 4).await;
    let outcome = &report.scenario_outcome;
    // Sequences 1 and 3 of each worker's 4 writes are refused.
    assert_eq!(outcome.refused_write_count, 6, "{outcome:?}");
    assert_eq!(outcome.lost_update_count, 0, "{outcome:?}");
    assert_eq!(outcome.error_count, 0, "{outcome:?}");
    assert!(report.passed(), "{outcome:?}");
    assert!(
        outcome
            .notes
            .iter()
            .any(|n| n.contains("refused 6 concurrent writes")
                && n.contains(
                    "REVISION_CONFLICT: the store changed after it was read. Read it again"
                )),
        "{:?}",
        outcome.notes
    );
}

#[tokio::test]
async fn a_refused_control_write_is_inconclusive_and_shows_the_error() {
    let report = run_store("closed", 2, 1).await;
    let outcome = &report.scenario_outcome;
    assert!(!report.passed(), "{outcome:?}");
    assert_eq!(outcome.refused_write_count, 0, "{outcome:?}");
    assert!(
        outcome
            .notes
            .iter()
            .any(|n| n.starts_with("control write: `append` returned isError: REVISION_CONFLICT")),
        "{:?}",
        outcome.notes
    );
}
