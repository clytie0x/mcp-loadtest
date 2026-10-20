//! Adversarial checks for the composite Action's private redaction mode.
//!
//! These spawn the real CLI because the contract covers both stdout and the
//! top-level error diagnostic written to stderr.

use std::path::Path;
use std::process::{Command, Output};

use tempfile::tempdir;

const SENTINEL: &str = "ACTION_SERVER_SECRET_7F3B";
const ARGS_SENTINEL: &str = "ACTION_ARG_SECRET_91C4";

fn run_cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mcp-loadtest"))
        .args(args)
        .output()
        .expect("spawn mcp-loadtest")
}

fn assert_redacted(output: &Output, server: &str) {
    assert!(!output.status.success(), "adversarial command should fail");
    for (stream, bytes) in [
        ("stdout", output.stdout.as_slice()),
        ("stderr", output.stderr.as_slice()),
    ] {
        let rendered = String::from_utf8_lossy(bytes);
        assert!(
            !rendered.contains(SENTINEL) && !rendered.contains(server),
            "{stream} leaked Action server identity:\n{rendered}"
        );
    }
}

fn assert_no_sentinel(output: &Output, sentinel: &str) {
    assert!(!output.status.success(), "adversarial command should fail");
    for (stream, bytes) in [
        ("stdout", output.stdout.as_slice()),
        ("stderr", output.stderr.as_slice()),
    ] {
        let rendered = String::from_utf8_lossy(bytes);
        assert!(
            !rendered.contains(sentinel),
            "{stream} leaked Action argument value:\n{rendered}"
        );
    }
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[test]
fn action_mode_redacts_deadlock_and_doctor_process_output() {
    let temp = tempdir().expect("tempdir");
    let output_dir = path_text(temp.path());
    let server = format!("no-such-action-binary --token {SENTINEL}");

    // Without `--tool` the failure comes from the tool-discovery session
    // (ADR 0030); with it, from the probe's own startup. Both are redacted.
    let discovery = run_cli(&[
        "deadlock-probe",
        "--server",
        &server,
        "--output-dir",
        &output_dir,
        "--action-redact-server-identity",
    ]);
    assert_redacted(&discovery, &server);

    let deadlock = run_cli(&[
        "deadlock-probe",
        "--server",
        &server,
        "--tool",
        "echo",
        "--concurrent",
        "1",
        "--hang-threshold",
        "1ms",
        "--grace-period",
        "1ms",
        "--output-dir",
        &output_dir,
        "--action-redact-server-identity",
    ]);
    assert_redacted(&deadlock, &server);

    let doctor = run_cli(&[
        "doctor",
        "--server",
        &server,
        "--runs-dir",
        &output_dir,
        "--action-redact-server-identity",
    ]);
    assert_redacted(&doctor, &server);
}

/// A child that prints its own argv to stderr and exits must not leak it
/// through the tool-discovery session either: Action mode discards that
/// session's stderr as it does the probe's.
#[test]
fn action_mode_discards_discovery_session_stderr() {
    let temp = tempdir().expect("tempdir");
    let output_dir = path_text(temp.path());
    let python = std::env::var("MCP_LOADTEST_PYTHON").unwrap_or_else(|_| "python".to_string());
    // `--server` is split on whitespace, so the script has none.
    let server = format!(
        "{python} -c __import__('sys').stderr.write(str(__import__('sys').argv)) {SENTINEL}"
    );

    let discovery = run_cli(&[
        "deadlock-probe",
        "--server",
        &server,
        "--output-dir",
        &output_dir,
        "--action-redact-server-identity",
    ]);
    assert_redacted(&discovery, &server);
}

#[test]
fn action_mode_redacts_malformed_tool_args_from_parse_errors() {
    let temp = tempdir().expect("tempdir");
    let output_dir = path_text(temp.path());

    let deadlock = run_cli(&[
        "deadlock-probe",
        "--server",
        "safe-unused-command",
        "--tool",
        "echo",
        "--args",
        ARGS_SENTINEL,
        "--output-dir",
        &output_dir,
        "--action-redact-server-identity",
    ]);
    assert_no_sentinel(&deadlock, ARGS_SENTINEL);
}

#[test]
fn action_mode_redacts_lost_update_server_and_flag_values() {
    let temp = tempdir().expect("tempdir");
    let output_dir = path_text(temp.path());
    let server = format!("no-such-action-binary --token {SENTINEL}");

    let lost_update = |server: &str, write_args: &str, env: &str| {
        run_cli(&[
            "lost-update",
            "--server",
            server,
            "--write-tool",
            "append",
            "--write-args",
            write_args,
            "--read-tool",
            "read",
            "--env",
            env,
            "--concurrent",
            "2",
            "--writes-per-worker",
            "1",
            "--output-dir",
            &output_dir,
            "--action-redact-server-identity",
        ])
    };

    let spawn = lost_update(&server, r#"{"item":"{marker}"}"#, "A=1");
    assert_redacted(&spawn, &server);

    let args = lost_update("safe-unused-command", ARGS_SENTINEL, "A=1");
    assert_no_sentinel(&args, ARGS_SENTINEL);

    let env = lost_update(
        "safe-unused-command",
        r#"{"item":"{marker}"}"#,
        ARGS_SENTINEL,
    );
    assert_no_sentinel(&env, ARGS_SENTINEL);
}

/// A child that prints its own argv to stderr must not leak it through
/// `lost-update` either, and a passing run's reports must omit the server.
#[test]
fn action_mode_redacts_lost_update_stderr_and_reports() {
    let temp = tempdir().expect("tempdir");
    let output_dir = path_text(&temp.path().join("runs"));
    let python = std::env::var("MCP_LOADTEST_PYTHON").unwrap_or_else(|_| "python".to_string());

    let echo_argv = format!(
        "{python} -c __import__('sys').stderr.write(str(__import__('sys').argv)) {SENTINEL}"
    );
    let crashed = run_cli(&[
        "lost-update",
        "--server",
        &echo_argv,
        "--write-tool",
        "append",
        "--write-args",
        r#"{"item":"{marker}"}"#,
        "--read-tool",
        "read",
        "--concurrent",
        "2",
        "--writes-per-worker",
        "1",
        "--output-dir",
        &output_dir,
        "--action-redact-server-identity",
    ]);
    assert_redacted(&crashed, &echo_argv);

    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../engine/tests/fixtures/mock-store.py");
    let store = temp.path().join("store");
    let server = format!(
        "{python} {} --mode safe --store {} {SENTINEL}",
        path_text(&fixture),
        path_text(&store)
    );
    let passed = run_cli(&[
        "lost-update",
        "--server",
        &server,
        "--write-tool",
        "append",
        "--write-args",
        r#"{"item":"{marker}"}"#,
        "--read-tool",
        "read",
        "--concurrent",
        "2",
        "--writes-per-worker",
        "2",
        "--output-dir",
        &output_dir,
        "--action-redact-server-identity",
    ]);
    assert!(
        passed.status.success(),
        "{}",
        String::from_utf8_lossy(&passed.stderr)
    );
    assert!(!String::from_utf8_lossy(&passed.stdout).contains(SENTINEL));
    assert!(!String::from_utf8_lossy(&passed.stderr).contains(SENTINEL));
    let mut report_files = 0;
    for run in std::fs::read_dir(&output_dir).expect("runs dir") {
        for file in std::fs::read_dir(run.expect("run").path()).expect("run dir") {
            let path = file.expect("report").path();
            let text =
                String::from_utf8_lossy(&std::fs::read(&path).expect("read report")).into_owned();
            assert!(
                !text.contains(SENTINEL),
                "{} leaked the server",
                path.display()
            );
            report_files += 1;
        }
    }
    assert!(report_files > 0, "the passing run wrote no reports");
}

/// The repro script embeds the server command, so Action mode refuses it.
#[test]
fn action_mode_refuses_emit_repro() {
    let temp = tempdir().expect("tempdir");
    let output_dir = path_text(&temp.path().join("runs"));
    let script = temp.path().join("repro.py");
    let server = format!("no-such-action-binary --token {SENTINEL}");
    let refused = run_cli(&[
        "lost-update",
        "--server",
        &server,
        "--write-tool",
        "append",
        "--write-args",
        r#"{"item":"{marker}"}"#,
        "--read-tool",
        "read",
        "--pipelined",
        "--emit-repro",
        &path_text(&script),
        "--output-dir",
        &output_dir,
        "--action-redact-server-identity",
    ]);
    assert_redacted(&refused, &server);
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("not available in Action mode"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(!script.exists(), "Action mode wrote a repro script");
}
