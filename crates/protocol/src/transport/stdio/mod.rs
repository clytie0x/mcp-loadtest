//! Stdio transport — newline-delimited JSON-RPC over a child process's
//! stdin/stdout.
//!
//! Pre-M4 this lived inline in `crate::Session`; refactored here so HTTP and
//! SSE can share the same `Session` orchestration.

use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{Transport, TransportError};

// The spawn path (process spawn + stderr-mode wiring) lives in `spawn.rs`, the
// stderr pump in `stderr_pump.rs`, and the bounded line reader (L-2 OOM guard)
// in `reader.rs`, and descendant cleanup in `tree.rs` — each a child module so
// this file stays under the 300-line production convention. See ADR 0013.
mod reader;
mod shutdown;
mod spawn;
mod stderr_pump;
mod tree;
use reader::read_bounded_line;

/// Default time budget applied around individual `request` calls when the
/// caller doesn't wrap them. Kept loose; finer control lives in the
/// scenario layer.
const DEFAULT_RECV_TIMEOUT: Duration = Duration::from_secs(60);

/// Hard cap on a single response line from the child. JSON-RPC has no spec
/// limit, but real MCP messages are < 1 MB; 16 MB leaves slack for pathological
/// servers while preventing a malicious server-under-test from OOM-ing the
/// load tester by emitting one unbounded line. On overflow we surface a
/// transport error rather than truncating silently.
const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

/// Upper bound on notification frames skipped while waiting for one response
/// (see `request`). Generous — real servers emit a handful of notifications
/// per response at most — but finite, so a server that streams notifications
/// forever surfaces a transport error instead of wedging the read.
const MAX_SKIPPED_NOTIFICATIONS: usize = 1024;

/// True when `line` is a JSON-RPC *notification* frame: a JSON object carrying
/// a `method` and no top-level `id`. Responses always echo the request `id`;
/// notifications never do. Anything that isn't this exact shape — a response
/// (has `id`), a malformed object with neither, or invalid JSON — is left for
/// the session layer, so its `IdMismatch` / malformed handling is unchanged.
///
/// Fast path: a notification must mention `"method"`, and responses almost
/// never do, so a substring miss short-circuits before any parse.
/// The reply to `line` if it is a request from the server: a frame with
/// both `method` and a non-null `id`. `ping` gets an empty result; any other
/// method gets method-not-found, since this client advertises no capability
/// a server could call back into (roots, sampling, elicitation).
fn server_request_reply(line: &str) -> Option<String> {
    if !line.contains("\"method\"") {
        return None;
    }
    #[derive(serde::Deserialize)]
    struct RequestProbe {
        #[serde(default)]
        id: Option<serde_json::Value>,
        #[serde(default)]
        method: Option<String>,
    }
    let probe: RequestProbe = serde_json::from_str(line).ok()?;
    let id = probe.id.filter(|id| !id.is_null())?;
    let method = probe.method?;
    let reply = if method == "ping" {
        serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": {} })
    } else {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": format!("method not found: {method}") }
        })
    };
    Some(reply.to_string())
}

fn is_notification_frame(line: &str) -> bool {
    if !line.contains("\"method\"") {
        return false;
    }
    #[derive(serde::Deserialize)]
    struct FrameProbe {
        #[serde(default)]
        id: Option<serde_json::Value>,
        #[serde(default)]
        method: Option<serde_json::Value>,
    }
    matches!(
        serde_json::from_str::<FrameProbe>(line),
        Ok(p) if p.id.is_none() && p.method.is_some()
    )
}

/// Stdio transport — owns the spawned child plus pipe handles.
///
/// `child` / `stdin` are `Option` only so `shutdown` can take them by value
/// for the EOF-then-`wait` teardown: the type implements [`Drop`] (stderr-pump
/// backstop) and a `Drop` type can't be destructured field-by-field. They are
/// `Some` for the whole live lifetime — the hot `request`/`notify` paths
/// `expect` them, which is infallible until `shutdown` consumes the transport.
pub struct StdioTransport {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    line_buf: Vec<u8>,
    /// Background task draining the child's stderr into a file (and optionally
    /// teeing to the parent's stderr). `None` when stderr is inherited (the
    /// default — no pump, zero overhead). Owned here so `shutdown`/`Drop` can
    /// stop it deterministically (mirrors the `ws`/`sse` reader task). The
    /// nested I/O result makes capture/read failures gate clean shutdown.
    stderr_pump: Option<JoinHandle<std::io::Result<()>>>,
    /// Cancels [`Self::stderr_pump`]. Always present so the field is uniform;
    /// a fresh token in the inherit case is simply never tripped.
    pump_cancel: CancellationToken,
    /// The child as spawned, so its descendants can be killed with it without
    /// trusting a PID that may since have been reused.
    identity: Option<tree::ProcessIdentity>,
}

impl StdioTransport {
    /// Sum of the internal graceful-exit, forced-reap, and stderr-pump phase
    /// budgets. Callers applying an outer timeout must add scheduling margin
    /// and keep their deadline strictly greater than this value.
    pub const SHUTDOWN_BUDGET: Duration = shutdown::SHUTDOWN_BUDGET;

    /// Borrow the live stdin. `None` is unreachable while the transport is in
    /// use (only `shutdown` takes it, consuming `self`); we still map it to a
    /// typed error instead of `expect()` to honour the no-panic-in-lib rule.
    fn stdin_mut(&mut self) -> Result<&mut ChildStdin, TransportError> {
        self.stdin.as_mut().ok_or(TransportError::Closed)
    }

    /// Read the next non-notification frame from stdout.
    async fn read_response(&mut self) -> Result<String, TransportError> {
        // Read the response line, with a wide default timeout so a wedged
        // server still surfaces eventually rather than hanging tests forever.
        //
        // A JSON-RPC server may interleave *notifications* (frames with a
        // `method` and no `id` — e.g. `notifications/tools/list_changed` or
        // progress updates) with responses at any time, including before the
        // `initialize` result. This transport is single-flight, so the next
        // line after a request isn't guaranteed to be its response: skip any
        // leading notification frames until the response arrives, rather than
        // mis-reading a notification and desyncing the stream for every
        // subsequent call. One `deadline` bounds the whole wait; a skip cap
        // stops a notification flood from wedging the read.
        //
        // Bound each read at `MAX_LINE_BYTES` (16 MB) via `read_bounded_line`
        // (`BufReader::read_line` has no byte cap) so a malicious or buggy
        // server-under-test can't OOM the load tester with one unbounded line.
        let deadline = tokio::time::Instant::now() + DEFAULT_RECV_TIMEOUT;
        for _ in 0..MAX_SKIPPED_NOTIFICATIONS {
            self.line_buf.clear();
            let read_fut = read_bounded_line(&mut self.stdout, &mut self.line_buf);
            let n = tokio::time::timeout_at(deadline, read_fut)
                .await
                .map_err(|_| TransportError::Timeout(DEFAULT_RECV_TIMEOUT))??;
            if n == 0 {
                return Err(TransportError::Closed);
            }
            // Invalid UTF-8 becomes U+FFFD so the JSON parser rejects the
            // frame instead of the whole transport aborting on one bad byte.
            let line = String::from_utf8_lossy(&self.line_buf);
            let line = line.trim_end();
            if is_notification_frame(line) {
                continue;
            }
            // A request from the server (`roots/list`, `ping`, ...) is
            // answered here; the server may hold our call until it is.
            if let Some(reply) = server_request_reply(line) {
                let stdin = self.stdin_mut()?;
                stdin.write_all(reply.as_bytes()).await?;
                stdin.write_all(b"\n").await?;
                stdin.flush().await?;
                continue;
            }
            return Ok(line.to_string());
        }
        Err(TransportError::Other(format!(
            "server sent {MAX_SKIPPED_NOTIFICATIONS}+ consecutive notifications with no response"
        )))
    }
}

#[async_trait]
impl Transport for StdioTransport {
    async fn request(&mut self, body: &str) -> Result<String, TransportError> {
        // Write framed line.
        let stdin = self.stdin_mut()?;
        stdin.write_all(body.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;

        self.read_response().await
    }

    async fn recv_response(&mut self) -> Result<String, TransportError> {
        self.read_response().await
    }

    async fn send_request(&mut self, body: &str) -> Result<(), TransportError> {
        self.notify(body).await
    }

    async fn notify(&mut self, body: &str) -> Result<(), TransportError> {
        let stdin = self.stdin_mut()?;
        stdin.write_all(body.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
        Ok(())
    }

    fn pid(&self) -> Option<u32> {
        self.child.as_ref().and_then(Child::id)
    }

    async fn shutdown(self: Box<Self>) -> Result<(), TransportError> {
        let mut this = self;
        shutdown::run(&mut this).await
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        // Backstop when the caller skipped `shutdown`. We can't `.await` in
        // `Drop`, so the pump flushes from a detached, time-bounded task (see
        // `stop_on_drop`). `kill_on_drop(true)` requests child termination;
        // graceful `shutdown` is the path that also waits for exit/reap.
        // Idempotent: after `shutdown` the pump is already `None` and the
        // token already cancelled, so this is a cheap no-op there.
        self.pump_cancel.cancel();
        if let Some(handle) = self.stderr_pump.take() {
            stderr_pump::stop_on_drop(handle, &self.pump_cancel);
        }
        // `kill_on_drop` reaches only the direct child; a launcher's server
        // must be found and killed before the launcher dies.
        if let Some(child) = self.child.as_mut()
            && matches!(child.try_wait(), Ok(None))
            && let Some(identity) = self.identity
        {
            tree::kill_descendants(identity);
        }
    }
}

// `read_bounded_line` (the L-2 OOM guard) lives in `reader.rs`, a child module
// declared above, so this file stays under the 300-line production
// convention. See ADR 0013.

// Pure classifier tests — no process spawn, so they run on every platform
// (unlike the unix-gated loopback below).
#[cfg(test)]
mod notification_tests {
    use super::{is_notification_frame, server_request_reply};

    #[test]
    fn server_requests_are_answered() {
        let ping = server_request_reply(r#"{"jsonrpc":"2.0","id":"s1","method":"ping"}"#)
            .expect("ping is a request");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&ping).expect("reply JSON"),
            serde_json::json!({ "jsonrpc": "2.0", "id": "s1", "result": {} })
        );
        let roots = server_request_reply(r#"{"method":"roots/list","jsonrpc":"2.0","id":0}"#)
            .expect("roots/list is a request");
        assert!(
            roots.contains(r#""id":0"#) && roots.contains("-32601"),
            "{roots}"
        );
        // Responses and notifications are not requests.
        assert!(server_request_reply(r#"{"jsonrpc":"2.0","id":3,"result":{}}"#).is_none());
        assert!(server_request_reply(r#"{"jsonrpc":"2.0","method":"notifications/x"}"#).is_none());
        assert!(server_request_reply(r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#).is_none());
    }

    #[test]
    fn notification_without_id_is_skipped() {
        assert!(is_notification_frame(
            r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#
        ));
        assert!(is_notification_frame(
            r#"{"method":"notifications/progress","params":{"progress":1},"jsonrpc":"2.0"}"#
        ));
    }

    #[test]
    fn response_with_id_is_not_a_notification() {
        // Ok result, error result, and a result whose payload text happens to
        // contain the word "method" — all carry an `id`, so none are skipped.
        assert!(!is_notification_frame(
            r#"{"jsonrpc":"2.0","id":3,"result":{}}"#
        ));
        assert!(!is_notification_frame(
            r#"{"jsonrpc":"2.0","id":3,"error":{"code":-32601,"message":"method not found"}}"#
        ));
        assert!(!is_notification_frame(
            r#"{"jsonrpc":"2.0","id":7,"result":{"content":[{"type":"text","text":"the method ran"}]}}"#
        ));
    }

    #[test]
    fn malformed_or_idless_response_is_left_for_the_session() {
        // Invalid JSON and an object with neither id nor method must NOT be
        // swallowed here — the session surfaces them as malformed.
        assert!(!is_notification_frame("not json at all"));
        assert!(!is_notification_frame(r#"{"jsonrpc":"2.0","result":{}}"#));
    }
}
