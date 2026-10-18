//! A server started through a launcher (`uvx`, `npx`, a shell script) is the
//! launcher's child, not ours. Shutting the session down must not leave it
//! running after the launcher is gone.
//!
//! The fixture launcher runs the server as a child sharing its stdio and waits
//! for it, as `uvx` does. The server answers MCP but ignores stdin EOF, so
//! shutdown must take the forced-kill path.

#[expect(
    dead_code,
    reason = "shared helpers module; this binary uses only python(), leaving fixture_path unused here"
)]
mod helpers;

use std::time::Duration;

use mcp_loadtest_protocol::Session;
use mcp_loadtest_protocol::mcp::Content;
use mcp_loadtest_protocol::transport::stdio::StdioTransport;
use serde_json::json;
use sysinfo::{Pid, ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System};

const TEST_TIMEOUT: Duration = Duration::from_secs(30);
const SHUTDOWN_SCHEDULING_MARGIN: Duration = Duration::from_secs(6);

const SERVER: &str = r#"
import sys, json, os, time
def send(o):
    sys.stdout.write(json.dumps(o) + "\n")
    sys.stdout.flush()
for line in sys.stdin:
    msg = json.loads(line)
    mid = msg.get("id")
    method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":mid,"result":{
            "protocolVersion":"2025-03-26",
            "capabilities":{"tools":{}},
            "serverInfo":{"name":"stubborn","version":"0.0.0"}}})
    elif method == "tools/call":
        send({"jsonrpc":"2.0","id":mid,"result":{
            "content":[{"type":"text","text":str(os.getpid())}]}})
    elif mid is not None:
        send({"jsonrpc":"2.0","id":mid,"error":{"code":-32601,"message":"method not found"}})
time.sleep(600)
"#;

const LAUNCHER: &str = r#"
import subprocess, sys
sys.exit(subprocess.call([sys.executable, "-c", sys.argv[1]]))
"#;

async fn spawn_launched_server() -> (Session, u32) {
    let py = helpers::python();
    let mut session =
        tokio::time::timeout(TEST_TIMEOUT, Session::spawn(&py, ["-c", LAUNCHER, SERVER]))
            .await
            .expect("spawn timed out")
            .expect("spawn failed");
    let result = session
        .call_tool("pid", &json!({}))
        .await
        .expect("pid call failed");
    let pid = match result.content.first() {
        Some(Content::Text { text }) => text.parse().expect("server pid"),
        other => panic!("unexpected pid content: {other:?}"),
    };
    assert_ne!(
        session.pid(),
        Some(pid),
        "the server must be the launcher's child"
    );
    (session, pid)
}

/// Gone or a zombie awaiting a reaper; either way it no longer runs.
async fn stopped_running(pid: u32) -> bool {
    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::new(),
        );
        match system.process(pid) {
            None => return true,
            Some(process) if process.status() == ProcessStatus::Zombie => return true,
            Some(_) if std::time::Instant::now() >= deadline => return false,
            Some(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
}

fn kill(pid: u32) {
    let mut system = System::new();
    let pid = Pid::from_u32(pid);
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::new(),
    );
    if let Some(process) = system.process(pid) {
        process.kill();
    }
}

#[tokio::test]
async fn forced_shutdown_kills_a_launched_server() {
    let (session, server_pid) = spawn_launched_server().await;

    tokio::time::timeout(
        StdioTransport::SHUTDOWN_BUDGET + SHUTDOWN_SCHEDULING_MARGIN,
        session.shutdown(),
    )
    .await
    .expect("shutdown timed out")
    .expect("shutdown errored");

    let stopped = stopped_running(server_pid).await;
    if !stopped {
        kill(server_pid);
    }
    assert!(
        stopped,
        "server PID {server_pid} outlived its launcher's shutdown"
    );
}

#[tokio::test]
async fn dropping_a_session_kills_a_launched_server() {
    let (session, server_pid) = spawn_launched_server().await;

    drop(session);

    let stopped = stopped_running(server_pid).await;
    if !stopped {
        kill(server_pid);
    }
    assert!(
        stopped,
        "server PID {server_pid} outlived its dropped session"
    );
}
