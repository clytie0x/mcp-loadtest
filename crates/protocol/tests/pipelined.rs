//! `Session::call_tools_pipelined` (ADR 0033) against a scripted stream:
//! every request goes out before any reply is read, replies are matched by
//! id in whatever order they arrive, and an unanswered call is reported
//! rather than waited on forever.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use mcp_loadtest_protocol::mcp::Content;
use mcp_loadtest_protocol::{Session, Transport, TransportError};
use serde_json::{Value, json};

/// Answers `initialize` through `request`; after that, `recv_response`
/// hands out `replies` in order, then blocks forever once they run out.
struct Stream {
    replies: VecDeque<String>,
    sent: Arc<Mutex<Vec<Value>>>,
}

#[async_trait]
impl Transport for Stream {
    async fn request(&mut self, body: &str) -> Result<String, TransportError> {
        let request: Value = serde_json::from_str(body).expect("request JSON");
        Ok(json!({
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "serverInfo": { "name": "stream", "version": "1" }
            }
        })
        .to_string())
    }

    async fn notify(&mut self, _body: &str) -> Result<(), TransportError> {
        Ok(())
    }

    async fn send_request(&mut self, body: &str) -> Result<(), TransportError> {
        self.sent
            .lock()
            .expect("sent lock")
            .push(serde_json::from_str(body).expect("request JSON"));
        Ok(())
    }

    async fn recv_response(&mut self) -> Result<String, TransportError> {
        match self.replies.pop_front() {
            Some(reply) => Ok(reply),
            None => std::future::pending().await,
        }
    }

    async fn shutdown(self: Box<Self>) -> Result<(), TransportError> {
        Ok(())
    }
}

fn reply(id: u64, text: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": { "content": [{ "type": "text", "text": text }] }
    })
    .to_string()
}

async fn session(replies: Vec<String>) -> (Session, Arc<Mutex<Vec<Value>>>) {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let stream = Stream {
        replies: replies.into(),
        sent: Arc::clone(&sent),
    };
    let session = Session::from_transport(stream).await.expect("initialize");
    (session, sent)
}

fn text(call: &mcp_loadtest_protocol::PipelinedCall) -> String {
    let result = call
        .result
        .as_ref()
        .expect("answered")
        .as_ref()
        .expect("ok");
    match &result.content[0] {
        Content::Text { text } => text.clone(),
        other => panic!("unexpected content {other:?}"),
    }
}

#[tokio::test]
async fn replies_out_of_order_are_matched_by_id() {
    // initialize used id 1; the three calls get ids 2, 3 and 4.
    let (mut session, sent) = session(vec![reply(4, "c"), reply(2, "a"), reply(3, "b")]).await;
    let (a, b, c) = (
        json!({ "v": "a" }),
        json!({ "v": "b" }),
        json!({ "v": "c" }),
    );
    let calls = [("w", &a), ("w", &b), ("w", &c)];
    let replies = session
        .call_tools_pipelined(&calls, tokio::time::Instant::now() + Duration::from_secs(5))
        .await
        .expect("batch");

    let sent = sent.lock().expect("sent lock");
    assert_eq!(sent.len(), 3, "every request goes out before any reply");
    let ids: Vec<u64> = sent.iter().map(|r| r["id"].as_u64().expect("id")).collect();
    assert_eq!(ids, [2, 3, 4]);
    assert_eq!(
        replies.iter().map(text).collect::<Vec<_>>(),
        ["a", "b", "c"],
        "results come back in call order"
    );
}

#[tokio::test]
async fn an_unanswered_call_is_reported_at_the_deadline() {
    let (mut session, _) = session(vec![reply(3, "b")]).await;
    let (a, b) = (json!({}), json!({}));
    let calls = [("w", &a), ("w", &b)];
    let replies = session
        .call_tools_pipelined(
            &calls,
            tokio::time::Instant::now() + Duration::from_millis(200),
        )
        .await
        .expect("batch");
    assert!(replies[0].result.is_none(), "id 2 never answered");
    assert_eq!(text(&replies[1]), "b");
}

#[tokio::test]
async fn a_reply_to_no_outstanding_call_fails_the_batch() {
    let (mut session, _) = session(vec![reply(99, "stray")]).await;
    let a = json!({});
    let err = session
        .call_tools_pipelined(
            &[("w", &a)],
            tokio::time::Instant::now() + Duration::from_secs(5),
        )
        .await
        .expect_err("id 99 matches nothing");
    assert!(
        err.to_string().contains("matches no outstanding request"),
        "{err}"
    );
}
