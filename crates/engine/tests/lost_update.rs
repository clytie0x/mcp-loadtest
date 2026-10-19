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
//! - `inplace`: truncates the store before writing it back, as
//!   server-filesystem 1.0.0 does; another process reading in that window
//!   drops data stored before the run, so the control write is lost.
//!
//! The pipelined mode (ADR 0033) sends each round's writes back to back on
//! one session. `--concurrent` makes the fixture handle them on threads, so
//! a racy store loses them inside one process; without it, the fixture
//! handles requests one at a time and nothing is lost. `stalling` never
//! answers odd-numbered writes, so the check must stop at that round.

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
    run_store_with(mode, &[], concurrent, writes_per_worker, false, "").await
}

/// `extra` goes after the fixture's arguments; `toml_tail` is appended to
/// the config (for example a `[thresholds]` table).
async fn run_store_with(
    mode: &str,
    extra: &[&str],
    concurrent: u32,
    writes_per_worker: u32,
    pipelined: bool,
    toml_tail: &str,
) -> Report {
    let scratch = ScratchDir::new(mode);
    let store = scratch.0.join("store");
    let mock = helpers::fixture_path("mock-store.py");
    let toml = format!(
        r#"
        [server]
        command = '{py}'
        args = ['{mock}', '--mode', '{mode}', '--store', '{store}'{extra}]
        [scenario]
        type = "lost_update"
        {toml_tail}
        "#,
        py = helpers::python(),
        mock = mock.display(),
        store = store.display(),
        extra = extra
            .iter()
            .map(|arg| format!(", '{arg}'"))
            .collect::<String>(),
    );
    let config = Config::from_toml_str(&toml).expect("config must parse");
    let scenario = Box::new(LostUpdate {
        concurrent,
        writes_per_worker,
        write_tool: "append".to_owned(),
        write_args: json!({ "item": "{marker}" }),
        read_tool: "read".to_owned(),
        read_args: json!({}),
        pipelined,
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

#[tokio::test]
async fn a_store_truncated_in_place_drops_data_it_already_held() {
    let report = run_store("inplace", 6, 4).await;
    let outcome = &report.scenario_outcome;
    assert!(outcome.lost_update_count > 0, "{outcome:?}");
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

#[tokio::test]
async fn pipelined_writes_race_inside_one_process() {
    let report = run_store_with("racy", &["--concurrent"], 5, 3, true, "").await;
    let outcome = &report.scenario_outcome;
    // Control write + read, 3 rounds of 5 writes, final read.
    assert_eq!(outcome.total_calls, 18, "{outcome:?}");
    assert!(
        outcome.lost_update_count > 0,
        "5 writes in flight on one racy process must lose some: {outcome:?}"
    );
    assert!(!report.passed(), "{outcome:?}");
}

#[tokio::test]
async fn pipelined_writes_to_a_one_at_a_time_server_all_land() {
    let report = run_store_with("racy", &[], 5, 3, true, "").await;
    let outcome = &report.scenario_outcome;
    assert_eq!(outcome.lost_update_count, 0, "{outcome:?}");
    assert_eq!(outcome.successful_calls, 18, "{outcome:?}");
    assert!(report.passed(), "{outcome:?}");
}

#[tokio::test]
async fn pipelined_mode_checks_a_server_that_keeps_state_in_memory() {
    // The default mode is inconclusive here (each session is a process);
    // one session sees its own writes.
    let report = run_store_with("memory", &["--concurrent"], 4, 2, true, "").await;
    let outcome = &report.scenario_outcome;
    assert_eq!(outcome.lost_update_count, 0, "{outcome:?}");
    assert!(report.passed(), "{outcome:?}");
}

#[tokio::test]
async fn pipelined_refusals_are_counted() {
    let report = run_store_with("refusing", &["--concurrent"], 3, 4, true, "").await;
    let outcome = &report.scenario_outcome;
    // Rounds 1 and 3 of 4 are refused: 2 rounds x 3 writes.
    assert_eq!(outcome.refused_write_count, 6, "{outcome:?}");
    assert_eq!(outcome.error_count, 0, "{outcome:?}");
    assert!(report.passed(), "{outcome:?}");
}

#[tokio::test]
async fn pipelined_writes_left_unanswered_stop_the_check() {
    let report = run_store_with(
        "stalling",
        &["--concurrent"],
        3,
        3,
        true,
        "[thresholds]\nhang_timeout = \"500ms\"",
    )
    .await;
    let outcome = &report.scenario_outcome;
    // Round 0 lands; round 1 is never answered; round 2 never goes out.
    assert_eq!(outcome.deadlock_count, 3, "{outcome:?}");
    assert_eq!(outcome.successful_calls, 5, "{outcome:?}");
    assert!(!report.passed(), "{outcome:?}");
    assert!(
        outcome
            .notes
            .iter()
            .any(|n| n.contains("3 pipelined writes still unanswered")),
        "{:?}",
        outcome.notes
    );
}
