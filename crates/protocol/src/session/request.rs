//! The request/response surface of [`Session`]: `list_tools`, `call_tool`,
//! the strict-schema setters, and the private `request`/`notify`
//! JSON-RPC plumbing.
//!
//! Split out of `session/mod.rs` to keep that file within the size
//! convention. `request`/`notify` are `pub(super)` so the sibling `lifecycle`
//! (initialize) and `connection` (discover) modules can drive the wire.

use std::collections::HashMap;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::{Session, SessionError, connection, strict};
use crate::jsonrpc::{
    JSONRPC_VERSION, OutgoingNotification, OutgoingRequest, ResponseEnvelope, ResponsePayload,
};
use crate::mcp::{
    CallToolParams, CallToolResult, ListToolsResult, ProtocolVersion, Tool, ToolCallRound,
};

fn response_shape_error(method: &str, detail: &str) -> SessionError {
    SessionError::ResponseShape(<serde_json::Error as serde::de::Error>::custom(format!(
        "stateless 2026-07-28 `{method}` result {detail}"
    )))
}

/// Validate the final stateless result envelope before method-specific serde
/// discards fields it does not currently expose. This is deliberately kept
/// out of the legacy handshake path: older revisions predate these fields.
fn validate_final_stateless_result(method: &str, result: &Value) -> Result<(), SessionError> {
    let object = result
        .as_object()
        .ok_or_else(|| response_shape_error(method, "must be an object"))?;

    match object.get("resultType") {
        Some(Value::String(result_type))
            if result_type == "complete"
                || (method == "tools/call" && result_type == "input_required") => {}
        Some(Value::String(result_type)) => {
            return Err(response_shape_error(
                method,
                &format!("has unsupported `resultType` value `{result_type}`"),
            ));
        }
        Some(_) => {
            return Err(response_shape_error(
                method,
                "field `resultType` must be the string `complete`",
            ));
        }
        // Backward compatibility required by the final spec: a tools/call
        // result without a discriminator is a complete result.
        None if method == "tools/call" => {}
        None => {
            return Err(response_shape_error(
                method,
                "is missing required field `resultType`",
            ));
        }
    }

    if matches!(method, "server/discover" | "tools/list") {
        match object.get("ttlMs") {
            Some(value) if value.as_u64().is_some() => {}
            Some(_) => {
                return Err(response_shape_error(
                    method,
                    "field `ttlMs` must be a non-negative integer",
                ));
            }
            None => {
                return Err(response_shape_error(
                    method,
                    "is missing required field `ttlMs`",
                ));
            }
        }

        match object.get("cacheScope") {
            Some(Value::String(scope)) if matches!(scope.as_str(), "private" | "public") => {}
            Some(Value::String(scope)) => {
                return Err(response_shape_error(
                    method,
                    &format!("has unsupported `cacheScope` value `{scope}`"),
                ));
            }
            Some(_) => {
                return Err(response_shape_error(
                    method,
                    "field `cacheScope` must be the string `private` or `public`",
                ));
            }
            None => {
                return Err(response_shape_error(
                    method,
                    "is missing required field `cacheScope`",
                ));
            }
        }
    }

    Ok(())
}

/// One call's outcome in [`Session::call_tools_pipelined`].
#[derive(Debug)]
pub struct PipelinedCall {
    /// The call's result, or `None` if no reply arrived before the deadline.
    pub result: Option<Result<CallToolResult, SessionError>>,
    /// Time from the start of the batch to this call's reply.
    pub elapsed: Duration,
}

impl Session {
    /// Call `tools/list` and return the server's tool registry.
    pub async fn list_tools(&mut self) -> Result<Vec<Tool>, SessionError> {
        let result: ListToolsResult = self.request("tools/list", &serde_json::json!({})).await?;
        Ok(result.tools)
    }

    /// Call `tools/list` for one page: the first page when `cursor` is
    /// `None`, otherwise the page after a previous `nextCursor`. Returns the
    /// page's tools and the server's `nextCursor`, `None` on the last page.
    ///
    /// [`Session::list_tools`] reads only the first page; callers that need
    /// every tool loop on this until the cursor is `None`.
    pub async fn list_tools_page(
        &mut self,
        cursor: Option<&str>,
    ) -> Result<(Vec<Tool>, Option<String>), SessionError> {
        let params = match cursor {
            Some(cursor) => serde_json::json!({ "cursor": cursor }),
            None => serde_json::json!({}),
        };
        let result: ListToolsResult = self.request("tools/list", &params).await?;
        Ok((result.tools, result.next_cursor))
    }

    /// Call `tools/list` until the server stops returning `nextCursor` and
    /// return every page's tools in the server's listed order.
    ///
    /// A cursor the server already returned is an error rather than a loop
    /// that only the caller's deadline would end.
    pub async fn list_all_tools(&mut self) -> Result<Vec<Tool>, SessionError> {
        let mut tools = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut cursor: Option<String> = None;
        loop {
            let (page, next) = self.list_tools_page(cursor.as_deref()).await?;
            tools.extend(page);
            let Some(next) = next else {
                return Ok(tools);
            };
            if !seen.insert(next.clone()) {
                return Err(SessionError::Transport(
                    crate::transport::TransportError::Other(format!(
                        "`tools/list` returned nextCursor `{next}` twice"
                    )),
                ));
            }
            cursor = Some(next);
        }
    }

    /// Turn on strict args validation for subsequent [`Session::call_tool`]
    /// calls. `schemas` maps each tool's name to its advertised
    /// `inputSchema`. Call once at run start (the caller already has the
    /// `tools/list` result in hand). A tool absent from `schemas` is never
    /// validated — a server that doesn't advertise a schema is not failed
    /// on that ground (forward-compatible, ADR 0005).
    pub fn set_strict_tool_schemas(&mut self, schemas: HashMap<String, Value>) {
        self.tool_schemas = Some(schemas);
    }

    /// Turn on result-side validation for subsequent [`Session::call_tool`]
    /// calls: each successful result's `structuredContent` is checked
    /// against the tool's advertised `outputSchema`. A tool absent from
    /// `schemas` is never result-validated (ADR 0005). Under the current
    /// policy mismatches — including a missing `structuredContent` — are
    /// **non-gating**: they warn and never fail the call (DESIGN §13.1).
    pub fn set_strict_tool_output_schemas(&mut self, schemas: HashMap<String, Value>) {
        self.tool_output_schemas = Some(schemas);
    }

    /// Call `tools/call` with the given tool name and arguments.
    ///
    /// Both `name` and `arguments` are borrowed — callers driving in a hot
    /// loop (sustained / pattern scenarios) can pass
    /// `&self.tool` and `&self.args` directly with zero per-iteration clone.
    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: &Value,
    ) -> Result<CallToolResult, SessionError> {
        // Opt-in strict args validation. When `tool_schemas` is `None`
        // (default) this is a single branch and the hot path is unchanged.
        if let Some(schemas) = &self.tool_schemas {
            strict::check_args(schemas, name, arguments)?;
        }

        let result = match self.call_tool_round(name, arguments, None, None).await? {
            ToolCallRound::Complete(result) => result,
            ToolCallRound::InputRequired(_) => {
                return Err(SessionError::AdditionalInputRequired);
            }
        };

        // Opt-in result-side validation (DESIGN §13.1 item 2). Same
        // single-`Option`-branch hot-path discipline as args (ADR 0006).
        // Non-gating under the current policy: `classify_schema_violation`
        // maps `ToolCallResult` to `Warn`, so mismatches log and never
        // alter or fail the returned result.
        if let Some(schemas) = &self.tool_output_schemas {
            strict::check_result(schemas, name, &result)?;
        }
        Ok(result)
    }

    /// Execute one multi-round `tools/call` exchange.
    ///
    /// For a continuation, pass the previous round's exact `requestState`
    /// and an object mapping each input-request key to its client result.
    /// Legacy servers simply return [`ToolCallRound::Complete`].
    pub async fn call_tool_round(
        &mut self,
        name: &str,
        arguments: &Value,
        request_state: Option<&str>,
        input_responses: Option<&Value>,
    ) -> Result<ToolCallRound, SessionError> {
        if let Some(responses) = input_responses
            && !responses.is_object()
        {
            return Err(SessionError::InvalidInputResponses);
        }
        let params = CallToolParams {
            name,
            arguments,
            request_state,
            input_responses,
        };
        self.request("tools/call", &params).await
    }

    /// Send every call in `calls` back to back on this session, then read
    /// the replies, matching them to calls by id in whatever order they
    /// arrive (ADR 0033). Each call is encoded and checked exactly as
    /// [`Session::call_tool`] would.
    ///
    /// Replies are read until `deadline`; a call still unanswered then has
    /// `result: None`. Its reply may still arrive later, so the session must
    /// not be used again after any `None`. A send or receive failure, or a
    /// reply whose id matches no outstanding call, fails the whole batch.
    /// Needs a stream transport ([`Transport::send_request`]).
    ///
    /// [`Transport::send_request`]: crate::transport::Transport::send_request
    pub async fn call_tools_pipelined(
        &mut self,
        calls: &[(&str, &Value)],
        deadline: tokio::time::Instant,
    ) -> Result<Vec<PipelinedCall>, SessionError> {
        let started = tokio::time::Instant::now();
        let mut replies: Vec<PipelinedCall> = calls
            .iter()
            .map(|_| PipelinedCall {
                result: None,
                elapsed: Duration::ZERO,
            })
            .collect();
        let mut outstanding: HashMap<u64, usize> = HashMap::new();
        for (index, (name, arguments)) in calls.iter().enumerate() {
            if let Some(schemas) = &self.tool_schemas
                && let Err(err) = strict::check_args(schemas, name, arguments)
            {
                replies[index].result = Some(Err(err));
                continue;
            }
            let params = CallToolParams {
                name,
                arguments,
                request_state: None,
                input_responses: None,
            };
            let (id, body) = self.encode_request("tools/call", &params)?;
            self.transport.send_request(&body).await?;
            outstanding.insert(id, index);
        }

        while !outstanding.is_empty() {
            let Ok(body) = tokio::time::timeout_at(deadline, self.transport.recv_response()).await
            else {
                break;
            };
            let env: ResponseEnvelope = serde_json::from_str(&body?)?;
            let Some(index) = env.id.as_u64().and_then(|id| outstanding.remove(&id)) else {
                return Err(SessionError::Transport(
                    crate::transport::TransportError::Other(format!(
                        "pipelined reply has id {} that matches no outstanding request",
                        env.id
                    )),
                ));
            };
            let id = env.id.as_u64().unwrap_or_default();
            let name = calls[index].0;
            let result = self
                .decode_response::<ToolCallRound>("tools/call", id, env)
                .and_then(|round| match round {
                    ToolCallRound::Complete(result) => Ok(result),
                    ToolCallRound::InputRequired(_) => Err(SessionError::AdditionalInputRequired),
                })
                .and_then(|result| {
                    if let Some(schemas) = &self.tool_output_schemas {
                        strict::check_result(schemas, name, &result)?;
                    }
                    Ok(result)
                });
            replies[index] = PipelinedCall {
                result: Some(result),
                elapsed: started.elapsed(),
            };
        }
        Ok(replies)
    }

    pub(super) async fn request<P, R>(
        &mut self,
        method: &str,
        params: &P,
    ) -> Result<R, SessionError>
    where
        P: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        self.request_skipping_stale(method, params, None).await
    }

    /// Like [`Session::request`], but first sets aside one late reply carrying
    /// the given id — an earlier request whose caller stopped waiting (the
    /// timed-out auto-negotiation probe). The set-aside body is stored in the
    /// slot so the caller can still act on it. Any other id mismatch still
    /// fails as before.
    pub(super) async fn request_skipping_stale<P, R>(
        &mut self,
        method: &str,
        params: &P,
        stale: Option<(u64, &mut Option<String>)>,
    ) -> Result<R, SessionError>
    where
        P: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let (id, body) = self.encode_request(method, params)?;
        let mut response_body = self.transport.request(&body).await?;
        if let Some((stale_id, slot)) = stale {
            let first: ResponseEnvelope = serde_json::from_str(&response_body)?;
            if first.id.as_u64() == Some(stale_id) {
                let late =
                    std::mem::replace(&mut response_body, self.transport.recv_response().await?);
                *slot = Some(late);
            }
        }

        let env: ResponseEnvelope = serde_json::from_str(&response_body)?;
        self.decode_response(method, id, env)
    }

    /// Serialize one request, consuming the next id.
    fn encode_request<P>(&mut self, method: &str, params: &P) -> Result<(u64, String), SessionError>
    where
        P: Serialize + ?Sized,
    {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);

        // Borrowed wrapper — `method` and `params` are referenced directly so
        // `serde_json::to_string` walks the caller's values without first
        // materializing an intermediate `Value` tree (the old code did
        // `serde_json::to_value(params)?` then re-serialized). The stateless
        // mode (ADR 0019) additionally flattens a `_meta` block next to the
        // params, still borrow-only.
        let body = match &self.stateless {
            Some(meta) => {
                let params = connection::WithMeta::new(params, meta);
                serde_json::to_string(&OutgoingRequest {
                    jsonrpc: JSONRPC_VERSION,
                    id,
                    method,
                    params: &params,
                })?
            }
            None => serde_json::to_string(&OutgoingRequest {
                jsonrpc: JSONRPC_VERSION,
                id,
                method,
                params,
            })?,
        };
        Ok((id, body))
    }

    /// Check one response envelope against the request `id` it answers and
    /// decode its result.
    fn decode_response<R>(
        &self,
        method: &str,
        id: u64,
        env: ResponseEnvelope,
    ) -> Result<R, SessionError>
    where
        R: DeserializeOwned,
    {
        if env.jsonrpc != JSONRPC_VERSION {
            return Err(SessionError::InvalidJsonRpcVersion { got: env.jsonrpc });
        }
        if env.id.as_u64() != Some(id) {
            return match env.payload {
                ResponsePayload::Ok { result } => Err(SessionError::MismatchedSuccessResponse {
                    expected: id,
                    got: env.id,
                    result,
                }),
                ResponsePayload::Err { error } => Err(SessionError::MismatchedErrorResponse {
                    expected: id,
                    got: env.id,
                    error,
                }),
            };
        }
        match env.payload {
            ResponsePayload::Ok { result } => {
                if self
                    .stateless
                    .as_ref()
                    .is_some_and(|meta| meta.version == ProtocolVersion::V2026_07_28)
                {
                    validate_final_stateless_result(method, &result)?;
                }
                serde_json::from_value(result).map_err(SessionError::ResponseShape)
            }
            ResponsePayload::Err { error } => Err(SessionError::Server(error)),
        }
    }

    pub(super) async fn notify<P: Serialize + ?Sized>(
        &mut self,
        method: &str,
        params: &P,
    ) -> Result<(), SessionError> {
        let notif = OutgoingNotification {
            jsonrpc: JSONRPC_VERSION,
            method,
            params,
        };
        let body = serde_json::to_string(&notif)?;
        self.transport.notify(&body).await?;
        Ok(())
    }
}
