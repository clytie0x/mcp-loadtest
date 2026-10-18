//! End-to-end serialize/parse cost of `Session::call_tool` against a fake
//! transport that echoes a canned successful JSON-RPC response.
//!
//! This isolates the *protocol* cost from any real I/O — no pipe, no TCP,
//! no SSE. What we measure:
//!
//! 1. `OutgoingRequest` serialize via `serde_json::to_string`.
//! 2. The "transport hop" (a tiny `parse id` + format response).
//! 3. `ResponseEnvelope` parse via `serde_json::from_str`.
//! 4. `CallToolResult` `from_value` deserialization.
//!
//! The `Session::call_tool(&str, &Value)` API takes borrowed args, so a
//! caller in a tight loop pays no allocation per iter beyond the request
//! body string. This bench reflects that.
//!
//! Uses a current-thread `tokio` runtime and `rt.block_on(...)` inside each
//! iter closure. We construct the runtime + `Session` *once* outside the
//! `iter` loop so we charge only per-call cost.

#![allow(missing_docs)]

use async_trait::async_trait;
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use mcp_loadtest_protocol::Session;
use mcp_loadtest_protocol::transport::{Transport, TransportError};
use serde_json::{Value, json};
use tokio::runtime::Runtime;

/// Build a fresh `Session` wired to a loopback transport. The `initialize`
/// handshake runs once during construction; the bench then drives
/// `call_tool` repeatedly against the same `LoopbackTransport`.
fn build_session(rt: &Runtime, result: String) -> Session {
    rt.block_on(async {
        Session::from_transport(LoopbackTransport {
            handshook: false,
            result,
        })
        .await
        .expect("loopback initialize must succeed")
    })
}

/// `tools/call` result member for the tiny/medium-args benches.
const EMPTY_RESULT: &str = r#""result":{"content":[],"isError":false}"#;

/// A `tools/call` result member with one text item of about `bytes` bytes,
/// mixing ASCII and multi-byte UTF-8 the way real tool output does.
fn text_result(bytes: usize) -> String {
    let unit = "price ราคา 450.5 ";
    let text = unit.repeat(bytes / unit.len() + 1);
    let item = json!({ "type": "text", "text": text });
    format!(r#""result":{{"content":[{item}],"isError":false}}"#)
}

/// Two-phase loopback. First inbound request (the `initialize` call issued
/// by `Session::from_transport`) returns an `InitializeResult` shape;
/// every subsequent request returns the `tools/call` success shape so the
/// `call_tool` benches see a tight serialize→parse loop with no I/O wait.
struct LoopbackTransport {
    handshook: bool,
    /// Pre-rendered `"result":{...}` member for every `tools/call` reply.
    result: String,
}

#[async_trait]
impl Transport for LoopbackTransport {
    async fn request(&mut self, body: &str) -> Result<String, TransportError> {
        let v: Value = serde_json::from_str(body)
            .map_err(|e| TransportError::Other(format!("init loopback parse failed: {e}")))?;
        let id = v.get("id").and_then(Value::as_u64).unwrap_or(0);
        if !self.handshook {
            self.handshook = true;
            // Mirror the `InitializeResult` shape the real client expects.
            Ok(format!(
                "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{{}}}}}}"
            ))
        } else {
            // Subsequent calls — used by `call_tool` benches.
            Ok(format!(
                "{{\"jsonrpc\":\"2.0\",\"id\":{id},{}}}",
                self.result
            ))
        }
    }

    async fn notify(&mut self, _body: &str) -> Result<(), TransportError> {
        Ok(())
    }

    async fn shutdown(self: Box<Self>) -> Result<(), TransportError> {
        Ok(())
    }
}

fn bench_call_tool_tiny_args(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current_thread runtime must build");
    let mut session = build_session(&rt, EMPTY_RESULT.to_owned());
    let args = json!({ "x": 1 });

    c.bench_function("call_tool_tiny_args", |b| {
        b.iter(|| {
            rt.block_on(async {
                let res = session
                    .call_tool(black_box("echo"), black_box(&args))
                    .await
                    .expect("loopback call_tool must succeed");
                black_box(res);
            });
        });
    });
}

fn bench_call_tool_medium_args(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current_thread runtime must build");
    let mut session = build_session(&rt, EMPTY_RESULT.to_owned());
    let args = json!({
        "ticker": "AAPL",
        "strike": 460,
        "expiry_days": 30,
        "spot": 450.5,
    });

    c.bench_function("call_tool_medium_args", |b| {
        b.iter(|| {
            rt.block_on(async {
                let res = session
                    .call_tool(black_box("price_option"), black_box(&args))
                    .await
                    .expect("loopback call_tool must succeed");
                black_box(res);
            });
        });
    });
}

fn bench_call_tool_text_result(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current_thread runtime must build");
    let args = json!({ "x": 1 });

    for kib in [1, 16] {
        let mut session = build_session(&rt, text_result(kib * 1024));
        c.bench_function(&format!("call_tool_text_result_{kib}k"), |b| {
            b.iter(|| {
                rt.block_on(async {
                    let res = session
                        .call_tool(black_box("read"), black_box(&args))
                        .await
                        .expect("loopback call_tool must succeed");
                    black_box(res);
                });
            });
        });
    }
}

criterion_group!(
    benches,
    bench_call_tool_tiny_args,
    bench_call_tool_medium_args,
    bench_call_tool_text_result
);
criterion_main!(benches);
