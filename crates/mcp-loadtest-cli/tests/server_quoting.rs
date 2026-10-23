//! `--server` honors single and double quotes (ADR 0037). These spawn the
//! real CLI so the quoted words reach the server process as one argument.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::tempdir;

fn python() -> String {
    std::env::var("MCP_LOADTEST_PYTHON").unwrap_or_else(|_| "python".to_string())
}

fn probe(server: &str, output_dir: &Path) -> Output {
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
        .output()
        .expect("spawn mcp-loadtest")
}

#[test]
fn a_quoted_path_with_spaces_reaches_the_server_as_one_argument() {
    let temp = tempdir().expect("tempdir");
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../engine/tests/fixtures");
    let dir = temp.path().join("server dir");
    std::fs::create_dir(&dir).expect("create dir");
    for file in ["mock-annotated.py", "_common.py"] {
        std::fs::copy(fixtures.join(file), dir.join(file)).expect("copy fixture");
    }
    let script = dir.join("mock-annotated.py");
    let server = format!("\"{}\" '{}'", python(), script.display());

    let output = probe(&server, &temp.path().join("runs"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "probe failed for {server}:\n{stderr}"
    );
}

#[test]
fn an_unterminated_quote_fails_before_spawning() {
    let temp = tempdir().expect("tempdir");
    let output = probe(&format!("{} -c 'print(1)", python()), temp.path());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains("server command has an unterminated ' quote"),
        "unexpected stderr:\n{stderr}"
    );
}
