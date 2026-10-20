//! `lost-update` without `--write-tool`, `--write-args` or `--read-tool`
//! fills in what is missing from the server's tools and their hints
//! (ADR 0035). These spawn the real CLI because the contract covers the
//! stderr lines, the error list, and the exit code.
//!
//! `mock-store.py --autopick` lists decoys around the one qualifying write
//! (`record`, nested arguments) and the preferred read (`list_items`); every
//! other tool errors, so a run that reaches a verdict called the right ones.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::tempdir;

fn fixture_server(extra: &str) -> String {
    let python = std::env::var("MCP_LOADTEST_PYTHON").unwrap_or_else(|_| "python".to_string());
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../engine/tests/fixtures/mock-store.py");
    format!("{python} {} {extra}", path.display())
}

/// Run `lost-update` against `mock-store.py <server_args>` with its store
/// under `dir`, 4 writers and 3 writes each, plus `extra` flags.
fn lost_update(dir: &Path, server_args: &str, extra: &[&str]) -> Output {
    let store = dir.join("store");
    let output_dir = dir.join("runs");
    Command::new(env!("CARGO_BIN_EXE_mcp-loadtest"))
        .args([
            "lost-update",
            "--server",
            &fixture_server(server_args),
            "--env",
            &format!("MOCK_STORE_PATH={}", store.display()),
            // Cold Python starts on a loaded runner can exceed 10 s.
            "--startup-timeout",
            "60s",
            "--concurrent",
            "4",
            "--writes-per-worker",
            "3",
            "--output-dir",
            &output_dir.to_string_lossy(),
        ])
        .args(extra)
        .output()
        .expect("spawn mcp-loadtest")
}

/// stdout (the terminal report) and stderr (choices and the error).
fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn wrote_a_report(dir: &Path) -> bool {
    std::fs::read_dir(dir.join("runs"))
        .map(|entries| {
            entries
                .flatten()
                .any(|run| run.path().join("metrics.json").is_file())
        })
        .unwrap_or(false)
}

const WRITE_CHOICE: &str = "no --write-tool given; writing with `record` (the server marks it \
    not read-only, not destructive and closed-world) and arguments built from its input schema: \
    {\"count\":1,\"entries\":[{\"item\":\"{marker}\",\"kind\":\"note\"}]}. These are the \
    server's own hints; pass --write-tool to choose.";

const READ_CHOICE: &str = "no --read-tool given; reading with `list_items` (the server marks it \
    read-only and it takes no arguments). Before a loss fails the run, it is confirmed by a \
    second read; pass --read-tool to choose.";

#[test]
fn picks_both_tools_and_fails_a_racy_store() {
    let dir = tempdir().expect("tempdir");
    let output = lost_update(
        dir.path(),
        "--mode racy --concurrent --autopick",
        &["--pipelined"],
    );
    let all = text(&output);
    assert_eq!(output.status.code(), Some(1), "{all}");
    assert!(all.contains(WRITE_CHOICE), "{all}");
    assert!(all.contains(READ_CHOICE), "{all}");
    assert!(
        all.contains("Writes are real: point the server at a disposable store."),
        "{all}"
    );
    assert!(all.contains("lost updates;"), "{all}");
    assert!(all.contains("confirmed by a recheck"), "{all}");
}

#[test]
fn picks_both_tools_and_passes_a_safe_store() {
    let dir = tempdir().expect("tempdir");
    let output = lost_update(dir.path(), "--mode safe --autopick", &[]);
    let all = text(&output);
    assert!(output.status.success(), "{all}");
    assert!(all.contains(WRITE_CHOICE), "{all}");
    assert!(all.contains(READ_CHOICE), "{all}");
    assert!(wrote_a_report(dir.path()));
}

#[test]
fn a_picked_read_that_caps_its_output_is_inconclusive_not_a_loss() {
    let dir = tempdir().expect("tempdir");
    let output = lost_update(
        dir.path(),
        "--mode safe --autopick --read-limit 3",
        &["--pipelined"],
    );
    let all = text(&output);
    assert_eq!(output.status.code(), Some(1), "{all}");
    assert!(all.contains("incomplete lost-update check"), "{all}");
    assert!(all.contains("could not confirm the loss"), "{all}");
    assert!(!all.contains("lost updates;"), "{all}");
}

#[test]
fn without_a_qualifying_write_lists_tools_and_runs_nothing() {
    let dir = tempdir().expect("tempdir");
    // Plain mock-store marks `append` non-destructive but not closed-world.
    let output = lost_update(dir.path(), "--mode safe", &[]);
    let all = text(&output);
    assert_eq!(output.status.code(), Some(1), "{all}");
    assert!(
        all.contains("no tool qualifies to be picked automatically as the write tool."),
        "{all}"
    );
    assert!(
        all.contains("  - append: write: not marked closed-world (openWorldHint: false)"),
        "{all}"
    );
    assert!(
        all.contains("  - read: write: marked read-only"),
        "only the write role is explained: {all}"
    );
    assert!(all.contains("Hint: pass --write-tool NAME"), "{all}");
    assert!(!wrote_a_report(dir.path()), "no check may run");
}

#[test]
fn a_named_write_tool_gets_arguments_from_its_schema() {
    let dir = tempdir().expect("tempdir");
    let output = lost_update(
        dir.path(),
        "--mode racy --concurrent",
        &[
            "--pipelined",
            "--write-tool",
            "append",
            "--read-tool",
            "read",
        ],
    );
    let all = text(&output);
    assert_eq!(output.status.code(), Some(1), "{all}");
    assert!(
        all.contains(
            "no --write-args given; writing to `append` with arguments built from its input \
             schema: {\"item\":\"{marker}\"}"
        ),
        "{all}"
    );
    assert!(!all.contains("no --read-tool given"), "{all}");
    // A named read is not rechecked.
    assert!(!all.contains("recheck"), "{all}");
    assert!(all.contains("lost updates;"), "{all}");
}

#[test]
fn write_args_without_write_tool_is_a_usage_error() {
    let dir = tempdir().expect("tempdir");
    let output = lost_update(
        dir.path(),
        "--mode safe",
        &["--write-args", r#"{"item":"{marker}"}"#],
    );
    let all = text(&output);
    // clap's usage-error exit code.
    assert_eq!(output.status.code(), Some(2), "{all}");
    assert!(all.contains("--write-tool"), "{all}");
    assert!(!wrote_a_report(dir.path()));
}
