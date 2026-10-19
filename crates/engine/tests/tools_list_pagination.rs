//! A run's `tools/list` registry follows `nextCursor`. `mock-annotated.py
//! --paged` lists three tools on the first page and `get_status` on the
//! second; a registry built from the first page alone would leave the probed
//! tool out of coverage and report the first page as unexercised.

mod helpers;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mcp_loadtest_core::config::Config;
use mcp_loadtest_engine::Run;
use mcp_loadtest_engine::scenario::deadlock_probe::DeadlockProbe;
use serde_json::json;

#[tokio::test]
async fn run_registry_includes_tools_from_every_page() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let runs = std::env::temp_dir().join(format!(
        "mcp-loadtest-tools-list-pages-{}-{nanos}",
        std::process::id()
    ));
    let toml = format!(
        r#"
        [server]
        command = '{py}'
        args = ['{mock}', '--paged']
        [scenario]
        type = "deadlock_probe"
        "#,
        py = helpers::python(),
        mock = helpers::fixture_path("mock-annotated.py").display(),
    );
    let config = Config::from_toml_str(&toml).expect("config must parse");
    let scenario = Box::new(DeadlockProbe {
        concurrent: 2,
        hang_threshold: Duration::from_secs(20),
        grace_period: Duration::from_secs(30),
        tool: "get_status".to_owned(),
        args: json!({}),
    });
    let report = tokio::time::timeout(
        Duration::from_secs(90),
        Run::new(config, scenario, runs.clone()).execute(),
    )
    .await;
    let _ = std::fs::remove_dir_all(&runs);
    let report = report.expect("run timed out").expect("run should complete");

    assert!(report.passed(), "{:?}", report.scenario_outcome);
    let coverage = report.coverage.expect("coverage");
    let mut registered = coverage.registered.clone();
    registered.sort();
    assert_eq!(
        registered,
        ["get_status", "search", "wipe", "write_note"],
        "registry covers both pages"
    );
    assert_eq!(coverage.exercised.get("get_status"), Some(&2));
}
