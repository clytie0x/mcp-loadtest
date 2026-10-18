# 29. Restart a stdio server that the discovery probe killed

Date: 2026-10-18
Status: Accepted (extends decision 1 of ADR 0023)

## Context

With `protocol_version = "auto"` (the default), a session first sends the
2026-07-28 `server/discover` probe and falls back to the legacy `initialize`
handshake only on protocol evidence: a JSON-RPC error other than
-32020..=-32022, or a probe timeout (ADR 0023).

Servers built on older Python MCP SDKs (around `mcp` 1.6–1.9, common among
servers published in the first half of 2025) do not answer an unknown
request with -32601. Request validation fails inside the SDK's receive loop
and the process exits. The probe then sees the connection close, which is
not protocol evidence, so the session failed. A field run against
`mcp-server-sqlite` 2025.4.25 failed this way on every session until
`protocol_version` was pinned.

The fallback cannot happen on the same connection: the process is gone.

## Decision

For the `stdio` transport in `auto` mode only, if connecting fails because
the connection closed before a session was established, start a fresh
process once and connect with the legacy handshake (2025-11-25, which the
server can negotiate down). A warning, printed once per process, says that
pinning `protocol_version` skips the probe and the extra start.

- An explicitly pinned revision never falls back. A pinned 2026-07-28 server
  that closes the connection still fails.
- There is one restart. If the legacy attempt also fails, that error is
  reported.
- Both attempts share the session's startup deadline.

## Alternatives considered

- **Keep failing and document the pin.** Correct but hostile: `auto` is the
  default, and the failure ("connection closed") does not point at the probe.
- **Remember the fallback per command for the rest of the run.** Halves the
  extra starts in pooled runs, but `cold_start` would then measure the first
  iteration with a crash and restart and the others without, which makes the
  first an outlier. Restarting every session keeps each measurement
  consistent, and the warning names the pin that removes the cost.
- **Probe with `initialize` first.** Changes the documented negotiation
  order for every server to accommodate a defect in some.

## Consequences

- `auto` reaches these servers. The cost is a second process start per
  session; for `cold_start` the measured time includes the failed first
  start, which is what `auto` really costs against such a server.
- A server that crashes on any first request is started twice before the
  error is reported.
- With stderr capture, the restart truncates the per-session log, so the
  first process's traceback is replaced by the second's.
- HTTP and SSE targets are unchanged: a remote server does not exit, and a
  failed probe there already falls back or fails on protocol evidence.
- The library constructor `Session::spawn` is unchanged; the restart lives
  in the engine, which owns the spawn configuration.
