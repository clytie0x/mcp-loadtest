//! `race-check` and `lost-update` subcommands against the engine's Python
//! fixtures: each passes a sound server, fails a broken one with the reason
//! in the error, and writes its reports.

use std::path::PathBuf;
use std::time::Duration;

use mcp_loadtest_cli::cmd_detect::{DetectTarget, run_lost_update, run_race_check};
use tempfile::tempdir;

fn target(fixture: &str, extra: &str, env: Vec<String>) -> DetectTarget {
    let python = std::env::var("MCP_LOADTEST_PYTHON").unwrap_or_else(|_| "python".to_string());
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../engine/tests/fixtures")
        .join(fixture);
    DetectTarget {
        server: format!("{python} {} {extra}", path.display()),
        env,
        // Cold Python starts on a loaded Windows runner can exceed 10 s.
        startup_timeout: Duration::from_secs(60),
    }
}

fn wrote_a_report(dir: &std::path::Path) -> bool {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .any(|run| run.path().join("metrics.json").is_file())
        })
        .unwrap_or(false)
}

#[tokio::test]
async fn race_check_passes_identical_responses_and_fails_divergent_ones() {
    let out = tempdir().expect("tempdir");
    run_race_check(
        &target("mock-normal.py", "", vec![]),
        "echo",
        3,
        r#"{"x":1}"#,
        out.path(),
    )
    .await
    .expect("a deterministic tool passes");
    assert!(wrote_a_report(out.path()));

    let err = run_race_check(
        &target("mock-process-id.py", "", vec![]),
        "process_id",
        3,
        "{}",
        out.path(),
    )
    .await
    .expect_err("per-process answers diverge");
    assert!(err.to_string().contains("divergence"), "{err:#}");
}

#[tokio::test]
async fn lost_update_fails_a_racy_store_and_passes_a_safe_one_via_env() {
    let dir = tempdir().expect("tempdir");
    let racy = dir.path().join("racy-store");
    let err = run_lost_update(
        &target(
            "mock-store.py",
            "--mode racy",
            vec![format!("MOCK_STORE_PATH={}", racy.display())],
        ),
        "append",
        r#"{"item":"{marker}"}"#,
        "read",
        "{}",
        4,
        3,
        &dir.path().join("runs"),
    )
    .await
    .expect_err("a racy store loses writes");
    assert!(err.to_string().contains("lost update"), "{err:#}");
    assert!(
        racy.exists(),
        "--env must reach the server: {}",
        racy.display()
    );

    run_lost_update(
        &target(
            "mock-store.py",
            "--mode safe",
            vec![format!(
                "MOCK_STORE_PATH={}",
                dir.path().join("safe-store").display()
            )],
        ),
        "append",
        r#"{"item":"{marker}"}"#,
        "read",
        "{}",
        4,
        3,
        &dir.path().join("runs"),
    )
    .await
    .expect("a safe store keeps every write");
    assert!(wrote_a_report(&dir.path().join("runs")));
}

#[tokio::test]
async fn lost_update_rejects_a_plan_without_a_marker() {
    let dir = tempdir().expect("tempdir");
    let err = run_lost_update(
        &target("mock-store.py", "--mode safe", vec![]),
        "append",
        r#"{"item":"fixed"}"#,
        "read",
        "{}",
        4,
        3,
        dir.path(),
    )
    .await
    .expect_err("write args without {marker} cannot detect anything");
    assert!(err.to_string().contains("{marker}"), "{err:#}");
}
