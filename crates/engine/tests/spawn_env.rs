//! `[server.env]` and `working_dir` must reach every spawned stdio server:
//! the run session and the factory-spawned pool sessions alike.
//!
//! `mock-env.py` answers `env_probe` with a tool error unless it sees the
//! configured marker variable and working directory, so a run that drops
//! either one records an error on every call.

mod helpers;

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mcp_loadtest_core::config::Config;
use mcp_loadtest_engine::Run;
use mcp_loadtest_engine::scenario::Scenario;
use mcp_loadtest_engine::scenario::sustained::Sustained;
use serde_json::json;

const TEST_TIMEOUT: Duration = Duration::from_secs(60);
const MARKER: &str = "spawn-env-marker";

/// Unique scratch dir under the OS temp dir, removed on drop.
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p = std::env::temp_dir().join(format!(
            "mcp-loadtest-spawn-env-{tag}-{}-{nanos}",
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

async fn run_env_probe(concurrent: u32, tag: &str) {
    let runs = ScratchDir::new(&format!("{tag}-runs"));
    let workdir = ScratchDir::new(&format!("{tag}-cwd"));
    let mock = helpers::fixture_path("mock-env.py");
    let toml = format!(
        r#"
        [server]
        command = '{py}'
        args = ['{mock}']
        working_dir = '{cwd}'
        [server.env]
        MCP_LOADTEST_ENV_MARKER = '{MARKER}'
        [scenario]
        type = "sustained"
        duration = "2s"
        concurrent = {concurrent}
        tool = "env_probe"
        "#,
        py = helpers::python(),
        mock = mock.display(),
        cwd = workdir.0.display(),
    );
    let config = Config::from_toml_str(&toml).expect("config must parse");
    let scenario: Box<dyn Scenario> = Box::new(Sustained {
        concurrent,
        duration: Duration::from_secs(2),
        tool: "env_probe".to_string(),
        args: json!({ "marker": MARKER, "cwd": workdir.0.display().to_string() }),
    });

    let report = tokio::time::timeout(
        TEST_TIMEOUT,
        Run::new(config, scenario, runs.0.clone()).execute(),
    )
    .await
    .expect("run timed out")
    .expect("run should complete");

    let throughput = &report.metrics.throughput;
    assert!(
        throughput.total_requests > 0,
        "expected calls to be made: {report:?}"
    );
    assert_eq!(
        throughput.successful_requests, throughput.total_requests,
        "every env_probe call must see the configured env var and working dir: {:?}",
        report.scenario_outcome
    );
}

#[tokio::test]
async fn run_session_gets_server_env_and_working_dir() {
    run_env_probe(1, "single").await;
}

#[tokio::test]
async fn pooled_sessions_get_server_env_and_working_dir() {
    run_env_probe(3, "pooled").await;
}
