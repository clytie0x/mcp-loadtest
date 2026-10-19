//! A server may send requests of its own (`roots/list`, `ping`, ...) and
//! hold the client's call until they are answered. `mock-roots.py` does
//! what mcp-shrimp-task-manager does: it sends both after
//! `notifications/initialized` and answers nothing else until it gets
//! replies. The stdio transport must answer them and keep waiting for its
//! own response.

mod helpers;

use std::time::Duration;

use mcp_loadtest_protocol::Session;
use serde_json::json;

#[tokio::test]
async fn server_requests_are_answered_while_a_call_waits() {
    let mock = helpers::fixture_path("mock-roots.py");
    let py = helpers::python();
    let run = async {
        let mut session = Session::spawn(&py, [mock.as_os_str()])
            .await
            .expect("spawn");
        let tools = session.list_tools().await.expect("list_tools");
        assert!(tools.iter().any(|t| t.name == "echo"), "{tools:?}");
        let result = session
            .call_tool("echo", &json!({}))
            .await
            .expect("call_tool");
        format!("{result:?}")
    };
    let rendered = tokio::time::timeout(Duration::from_secs(60), run)
        .await
        .expect("the server's requests must be answered, or it never replies");
    assert!(
        rendered.contains("roots=-32601 ping=ok"),
        "roots/list gets method-not-found and ping an empty result: {rendered}"
    );
}
