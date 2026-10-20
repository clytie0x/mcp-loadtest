//! `lost-update --emit-repro` (ADR 0034) against `mock-store.py`.
//!
//! The default tests check when the script is written. The two ignored
//! tests run a generated script on the official SDK against the racy store
//! and expect it to report the loss, as it would for an upstream report.
//!
//! ## Running the ignored tests
//!
//! They need Node.js with npm, or a Python 3.10+ that can create a venv,
//! and network access to install the SDK unless one is supplied:
//!
//! ```bash
//! # optional: reuse installed SDKs instead of installing into a temp dir
//! export MCP_LOADTEST_REPRO_NODE_DIR=/path/with/node_modules/@modelcontextprotocol/sdk
//! export MCP_LOADTEST_REPRO_PYTHON=/path/to/venv/bin/python   # has `mcp`
//! cargo test -p mcp-loadtest-cli --test repro_script -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mcp_loadtest_cli::cmd_detect::{DetectTarget, run_lost_update};
use mcp_loadtest_cli::repro::{ReproRequest, ReproSdk};
use tempfile::tempdir;

fn python() -> String {
    std::env::var("MCP_LOADTEST_PYTHON").unwrap_or_else(|_| "python".to_string())
}

fn store_target(mode: &str, store: &Path) -> DetectTarget {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../engine/tests/fixtures/mock-store.py");
    DetectTarget {
        server: format!("{} {} --mode {mode}", python(), fixture.display()),
        env: vec![format!("MOCK_STORE_PATH={}", store.display())],
        // Cold Python starts on a loaded Windows runner can exceed 10 s.
        startup_timeout: Duration::from_secs(60),
        redact_server_identity: false,
    }
}

/// Run a pipelined `lost-update` with `--emit-repro script`.
async fn check(mode: &str, store: &Path, script: &Path, sdk: ReproSdk) -> anyhow::Result<()> {
    let request = ReproRequest::new(script.to_path_buf(), Some(sdk))?;
    run_lost_update(
        &store_target(mode, store),
        "append",
        r#"{"item":"{marker}"}"#,
        "read",
        "{}",
        5,
        2,
        true,
        Some(&request),
        &store.with_extension("runs"),
    )
    .await
}

#[tokio::test]
async fn a_repro_is_written_only_when_writes_are_lost() {
    let dir = tempdir().expect("tempdir");
    let script = dir.path().join("repro.mjs");
    let err = check(
        "racy --concurrent",
        &dir.path().join("racy"),
        &script,
        ReproSdk::Node,
    )
    .await
    .expect_err("a racy store loses writes");
    assert!(err.to_string().contains("lost update"), "{err:#}");
    let text = std::fs::read_to_string(&script).expect("repro written");
    assert!(
        text.contains(r#"name: "append", arguments: { "item": m }"#),
        "{text}"
    );
    assert!(text.contains("MOCK_STORE_PATH"), "{text}");
    assert!(text.contains("const CALLS = 5;"), "{text}");

    // A script left by an earlier failing run must not outlive a passing one.
    let script = dir.path().join("safe.py");
    std::fs::write(&script, "# stale").expect("stale script");
    check(
        "safe --concurrent",
        &dir.path().join("safe"),
        &script,
        ReproSdk::Python,
    )
    .await
    .expect("a safe store keeps every write");
    assert!(!script.exists(), "no finding, no script");
}

#[tokio::test]
async fn emitting_a_repro_needs_the_pipelined_mode() {
    let dir = tempdir().expect("tempdir");
    let request = ReproRequest::new(dir.path().join("repro.py"), None).expect("sdk from extension");
    let err = run_lost_update(
        &store_target("safe", &dir.path().join("store")),
        "append",
        r#"{"item":"{marker}"}"#,
        "read",
        "{}",
        4,
        1,
        false,
        Some(&request),
        dir.path(),
    )
    .await
    .expect_err("the script only reproduces the one-process case");
    assert!(err.to_string().contains("--pipelined"), "{err:#}");
}

/// Run `program args` in `dir` and return its exit code and stdout.
fn run(program: &str, args: &[&str], dir: &Path) -> (Option<i32>, String) {
    let out = Command::new(program)
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("running {program}: {e}"));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    eprintln!("{stdout}{}", String::from_utf8_lossy(&out.stderr));
    (out.status.code(), stdout)
}

/// The script's verdict on the racy store: exit 1 and a `LOST:` line, with
/// every call acknowledged.
fn assert_reports_the_loss((code, stdout): (Option<i32>, String)) {
    assert_eq!(code, Some(1), "{stdout}");
    assert!(stdout.contains("calls: 5 sent, 5 succeeded"), "{stdout}");
    assert!(stdout.contains("LOST: "), "{stdout}");
}

#[tokio::test]
#[ignore = "needs Node.js and npm; installs @modelcontextprotocol/sdk"]
async fn node_repro_reports_the_loss() {
    let scratch = tempdir().expect("tempdir");
    // Node resolves the SDK from the script's directory, so the script goes
    // next to node_modules.
    let sdk_dir = match std::env::var("MCP_LOADTEST_REPRO_NODE_DIR") {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => {
            let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
            let dir = scratch.path().join("node");
            std::fs::create_dir_all(&dir).expect("node dir");
            let (code, _) = run(
                npm,
                &["install", "--no-save", "@modelcontextprotocol/sdk@1.32.1"],
                &dir,
            );
            assert_eq!(code, Some(0), "npm install failed");
            dir
        }
    };
    let script = sdk_dir.join(format!("mlt-repro-{}.mjs", std::process::id()));
    let _ = check(
        "racy --concurrent",
        &scratch.path().join("store"),
        &script,
        ReproSdk::Node,
    )
    .await;
    assert!(script.exists(), "the run found no loss to reproduce");
    let result = run("node", &[script.to_str().expect("utf-8 path")], &sdk_dir);
    let _ = std::fs::remove_file(&script);
    assert_reports_the_loss(result);
}

#[tokio::test]
#[ignore = "needs Python 3.10+ with venv; installs the mcp package"]
async fn python_repro_reports_the_loss() {
    let scratch = tempdir().expect("tempdir");
    let interpreter = match std::env::var("MCP_LOADTEST_REPRO_PYTHON") {
        Ok(python) => python,
        Err(_) => {
            let venv = scratch.path().join("venv");
            let (code, _) = run(
                &python(),
                &["-m", "venv", venv.to_str().expect("utf-8 path")],
                scratch.path(),
            );
            assert_eq!(code, Some(0), "venv failed");
            let bin = if cfg!(windows) {
                "Scripts/python.exe"
            } else {
                "bin/python"
            };
            let interpreter = venv.join(bin).display().to_string();
            let (code, _) = run(
                &interpreter,
                &["-m", "pip", "install", "--quiet", "mcp"],
                scratch.path(),
            );
            assert_eq!(code, Some(0), "pip install mcp failed");
            interpreter
        }
    };
    let script = scratch.path().join("repro.py");
    let _ = check(
        "racy --concurrent",
        &scratch.path().join("store"),
        &script,
        ReproSdk::Python,
    )
    .await;
    assert!(script.exists(), "the run found no loss to reproduce");
    let result = run(
        &interpreter,
        &["-I", script.to_str().expect("utf-8 path")],
        scratch.path(),
    );
    assert_reports_the_loss(result);
}
