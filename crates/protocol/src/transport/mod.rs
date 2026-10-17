//! Transport abstraction — stdio / HTTP / SSE plug into [`crate::Session`]
//! through the same trait.
//!
//! See DESIGN.md §4 (architecture) and §6 (CLI surface).
//!
//! **M4 ownership:** integration agent owns this module + `stdio.rs`.
//! Agent J fills in `http.rs`. Agent K fills in `sse.rs`. Locked contracts:

pub mod guard;
pub mod headers;
pub mod http;
pub(crate) mod resolve;
pub mod spawn_options;
pub mod sse;
pub mod stdio;

pub use guard::HostGuard;

use std::io;

use async_trait::async_trait;
use thiserror::Error;

/// Errors a [`Transport`] may surface.
///
/// **Locked for M4.** New variants are non-breaking only if added at the end.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TransportError {
    /// I/O error from the underlying pipe / socket.
    #[error("io: {0}")]
    Io(#[from] io::Error),
    /// HTTP error (status code, reqwest failure, etc.).
    #[error("http: {0}")]
    Http(String),
    /// Server closed the connection mid-call.
    #[error("connection closed")]
    Closed,
    /// Configured deadline exceeded.
    #[error("timeout after {0:?}")]
    Timeout(std::time::Duration),
    /// Other transport-specific failure.
    #[error("{0}")]
    Other(String),
}

/// One end of a JSON-RPC link to an MCP server. Stdio / HTTP / SSE all
/// implement this; [`crate::Session`] is generic over `Box<dyn Transport>`.
///
/// **Locked for M4.** Method signatures are stable across patch versions.
#[async_trait]
pub trait Transport: Send {
    /// Send a request body (single JSON-RPC object, no trailing newline) and
    /// await the matching response body. Implementations are responsible for
    /// any wire-format wrapping (newline framing for stdio, POST + body for
    /// HTTP, POST + SSE filter for SSE).
    ///
    /// The returned string is the raw JSON-RPC response object (no framing).
    async fn request(&mut self, body: &str) -> Result<String, TransportError>;

    /// Send a notification body (no `id`, no response expected).
    async fn notify(&mut self, body: &str) -> Result<(), TransportError>;

    /// Read the next response frame **without** sending anything.
    ///
    /// Used to drain the late reply to a request whose caller stopped
    /// waiting (the auto-negotiation `server/discover` probe times out, then
    /// `initialize` goes out on the same stream). Only stream transports can
    /// hold such a reply; request/response transports (HTTP / SSE)
    /// inherit this default, which reports it as unsupported.
    async fn recv_response(&mut self) -> Result<String, TransportError> {
        Err(TransportError::Other(
            "recv_response unsupported on this transport".into(),
        ))
    }

    /// PID of the underlying process if applicable. Stdio knows; HTTP / SSE
    /// don't (would require a server-reported field, which MCP doesn't have).
    fn pid(&self) -> Option<u32> {
        None
    }

    /// Record the negotiated MCP protocol version once `initialize`
    /// completes. Transports that carry the version out-of-band override
    /// this — Streamable HTTP attaches the `MCP-Protocol-Version` header
    /// (required from the 2025-06-18 revision) to every subsequent request.
    /// The default is a no-op for wire formats with no such channel
    /// (stdio / SSE / WS).
    fn set_protocol_version(&mut self, _version: &str) {}

    /// Close the transport gracefully. Implementations are bounded by their
    /// own internal timeouts — the orchestrator wraps this in an outer
    /// timeout via `tokio::time::timeout` already.
    async fn shutdown(self: Box<Self>) -> Result<(), TransportError>;
}
