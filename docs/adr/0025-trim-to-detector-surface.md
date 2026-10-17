# 25. Trim the workspace to the detector surface

Date: 2026-10-17
Status: Accepted (applies [0024](0024-reposition-as-mcp-bug-detector.md))

## Context

[0024](0024-reposition-as-mcp-bug-detector.md) made mcp-loadtest a CI gate for
MCP concurrency bugs and dropped feature parity with load tools as a goal. The
workspace still carried the parity features built under
[0004](0004-compete-with-reaatech.md). A line count on 2026-10-17 put the
detectors (`deadlock_probe`, `race_check`, `hang_detector`) at about 5% of the
49,947 lines of Rust; the shared run engine, protocol, and config layers they
need were another 39%.

Generic load tools already cover the load-shape features. `xk6-mcp` drives MCP
servers from k6 over stdio, SSE, and Streamable HTTP with per-method metrics,
and `reaatech/mcp-load-test` ships breaking-point detection and reports. None
of the Phase 1 field runs used a load shape other than `sustained`, and none
used WebSocket.

## Decision

Remove the features that neither detect MCP concurrency bugs nor support the
detectors:

| Removed | Why |
|---|---|
| `ramp` + breaking-point analysis, `spike`, `soak` | Generic load shapes covered by k6 / xk6-mcp |
| `fuzzer`, `fuzz_report`, `Transport::raw_send`, `CallOutcome::ExpectedRejection` | Parser-bug class, outside the 0024 scope |
| WebSocket transport | Not a standard MCP transport; no integration tests, no field use |
| `cross` subcommand + A-F grading | Grading had no other caller; `compare` covers regression gating |
| HTML report | JSON, Markdown, JUnit, and Prometheus cover CI and humans |
| Ratatui TUI | No CLI entry point ever called it |
| `[output.history]` rolling baseline | Overlaps `compare` against a stored baseline |

Kept, because the detectors depend on them: `sustained` and the session pool
(latency baseline that separates a hang from a slow server), `cold_start`
(Phase 1 showed start-up latency is the main false-positive risk), `pattern`
(multi-step sequences needed for lost-update checks), `version_matrix`,
trace record/replay (response-divergence detector), HTTP/SSE with OAuth (hosted
servers), and the RSS leak threshold (its regression helper moved from the
`soak` module to `engine::leak_detect`).

## Consequences

- About 11,000 lines of Rust and the `ratatui`, `crossterm`,
  `tokio-tungstenite`, `tokio-stream`, and `indicatif` dependencies are gone.
- Breaking for configs and the Action: the removed scenario types, `ws`
  transport, `html` format, `[output.history]`, and the Action's
  `command: cross` are rejected with an error rather than ignored. This ships
  in `v0.3.0`.
- `metrics.json` no longer emits `expected_rejection_count`; it was omitted
  when zero and only the fuzzer produced it.
- A removed feature returns only with a real server or real bug that needs it,
  as 0024 requires for any new feature.
