//! `deadlock-probe` without `--tool` picks a tool the server marks read-only
//! that takes no required arguments (ADR 0030). These spawn the real CLI
//! because the contract covers the stderr line, the error list, and the exit
//! code.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::tempdir;

fn fixture_server(fixture: &str, extra: &str) -> String {
    let python = std::env::var("MCP_LOADTEST_PYTHON").unwrap_or_else(|_| "python".to_string());
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../engine/tests/fixtures")
        .join(fixture);
    format!("{python} {} {extra}", path.display())
}

/// Run `deadlock-probe` with generous thresholds (cold Python on a loaded
/// runner) plus `extra` flags.
fn probe(server: &str, output_dir: &Path, extra: &[&str]) -> Output {
    let output_dir = output_dir.to_string_lossy().into_owned();
    Command::new(env!("CARGO_BIN_EXE_mcp-loadtest"))
        .args([
            "deadlock-probe",
            "--server",
            server,
            "--concurrent",
            "2",
            "--hang-threshold",
            "20s",
            "--grace-period",
            "30s",
            "--output-dir",
            &output_dir,
        ])
        .args(extra)
        .output()
        .expect("spawn mcp-loadtest")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn wrote_a_report(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .any(|run| run.path().join("metrics.json").is_file())
        })
        .unwrap_or(false)
}

const CHOICE_LINE: &str = "no --tool given; probing `get_status` (the server marks it read-only \
                           and it takes no required arguments). These are the server's own \
                           hints; pass --tool to choose.";

#[test]
fn without_tool_probes_the_first_read_only_no_argument_tool() {
    let out = tempdir().expect("tempdir");
    // The fixture errors on every tool but `get_status`, so a pass means
    // the probe called it.
    let output = probe(&fixture_server("mock-annotated.py", ""), out.path(), &[]);
    let err = stderr(&output);
    assert!(output.status.success(), "probe failed:\n{err}");
    assert!(err.contains(CHOICE_LINE), "missing choice line:\n{err}");
    assert!(wrote_a_report(out.path()));
}

#[test]
fn without_tool_follows_tools_list_pagination() {
    let out = tempdir().expect("tempdir");
    let output = probe(
        &fixture_server("mock-annotated.py", "--paged"),
        out.path(),
        &[],
    );
    let err = stderr(&output);
    assert!(output.status.success(), "probe failed:\n{err}");
    assert!(err.contains(CHOICE_LINE), "missing choice line:\n{err}");
}

#[test]
fn without_tool_and_no_qualifying_tool_lists_tools_and_fails() {
    let out = tempdir().expect("tempdir");
    // mock-normal's only tool, `echo`, carries no annotations.
    let output = probe(&fixture_server("mock-normal.py", ""), out.path(), &[]);
    let err = stderr(&output);
    // The CLI's exit code for every non-clap error, bad --args JSON included.
    assert_eq!(output.status.code(), Some(1), "stderr:\n{err}");
    assert!(err.contains("no tool qualifies"), "{err}");
    assert!(
        err.contains("A tool qualifies only when the server marks it read-only and it has no required arguments."),
        "{err}"
    );
    assert!(
        err.contains("  - echo: not read-only; required arguments: none"),
        "{err}"
    );
    assert!(
        err.contains("Hint: pass --tool NAME and --args '{...}'."),
        "{err}"
    );
    assert!(!wrote_a_report(out.path()), "no probe may run");
}

#[test]
fn args_without_tool_is_a_usage_error() {
    let out = tempdir().expect("tempdir");
    let output = probe(
        &fixture_server("mock-normal.py", ""),
        out.path(),
        &["--args", "{}"],
    );
    let err = stderr(&output);
    // clap's usage-error exit code.
    assert_eq!(output.status.code(), Some(2), "stderr:\n{err}");
    assert!(err.contains("--tool"), "{err}");
    assert!(!wrote_a_report(out.path()));
}

#[test]
fn explicit_tool_echo_probes_as_before() {
    let out = tempdir().expect("tempdir");
    let output = probe(
        &fixture_server("mock-normal.py", ""),
        out.path(),
        &["--tool", "echo", "--args", r#"{"x":1}"#],
    );
    let err = stderr(&output);
    assert!(output.status.success(), "probe failed:\n{err}");
    assert!(!err.contains("no --tool given"), "{err}");
    assert!(wrote_a_report(out.path()));
}
