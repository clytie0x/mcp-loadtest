# 33. Pipelined lost-update check on one session

Date: 2026-10-19
Status: Accepted

## Context

`lost_update` (ADR [0027](0027-lost-update-detector.md)) drives each writer
through its own session. For a stdio target each session is its own server
process, so the check only sees races between processes that share a store.

The 2026-10-19 file-backed field run ([field-results.md](../field-results.md))
found that the common case is narrower. server-filesystem and mcp-taskmanager
lose writes inside one process as soon as a client sends tool calls without
waiting for each reply: with 2, 5 or 10 such calls, one survived. Clients
that run tool calls in parallel do exactly this. The run needed a separate
script to show it, because a `Session` sends one request and waits for its
response before the next.

## Decision

Add a pipelined mode to `lost_update` (`pipelined = true` in config,
`--pipelined` on the CLI, `pipelined` on the `serve --mcp` tool).

- **One session, one server process.** No pool is started. The control write
  and both reads use the run session, so the control proves only that the
  read observes writes; sharing state is given.
- **Rounds of in-flight writes.** Each of `writes_per_worker` rounds writes
  `concurrent` requests back to back, then reads the replies. `{worker}` is
  the request's slot in the round and `{seq}` the round, so markers keep
  their meaning.
- **Replies matched by id.** `Session::call_tools_pipelined` encodes the
  requests exactly as `call_tool` does (stateless `_meta`, strict argument
  checks, result checks), sends them through a new
  `Transport::send_request`, and reads replies through `recv_response` until
  every id has answered. Replies may arrive in any order. An unknown id, or a
  repeated one, is a protocol error for the round.
- **One deadline per round.** The round gets `hang_threshold + grace_period`.
  A reply later than `hang_threshold` counts as a hang. Requests still
  unanswered at the deadline count as deadlocks, and the check stops there:
  the stream still holds their replies, so nothing later on that session can
  be trusted.
- **Stream transports only.** `send_request` is implemented for stdio. HTTP
  and SSE keep the default, which reports pipelining as unsupported; a
  `server_url` target already shares one server process across sessions in
  the default mode, so it does not need this one.
- Verdicts, `refused_write_count` (ADR [0032](0032-lost-update-refused-writes.md))
  and the reported counts are the same as in the default mode.

## Alternatives considered

- **Multiplex `Session` for real concurrency** (a reader task routing replies
  to waiting callers). It would serve every scenario, but it changes the
  session's ownership model, which every scenario and transport depends on.
  The batch-and-match method covers the one scenario that needs it.
- **A stdio-to-HTTP bridge in front of the server.** It would let the
  existing mode share one process, at the cost of a second program between
  the tool and the server whose own queuing would decide what the server
  sees.
- **Make pipelining the default.** The cross-process case still matters (two
  clients on one workspace tore server-filesystem's file), so the default
  stays as it was.

## Consequences

- A stdio server that handles requests one at a time passes the pipelined
  mode even if it is racy across processes. Run both modes for a full
  picture.
- A pipelined deadlock leaves the run incomplete instead of continuing, since
  the session cannot be reused.
