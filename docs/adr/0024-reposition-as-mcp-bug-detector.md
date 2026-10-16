# 24. Strategy: reposition as an MCP concurrency-bug CI gate

Date: 2026-10-17
Status: Accepted (supersedes [0004](0004-compete-with-reaatech.md))

## Context

[0004](0004-compete-with-reaatech.md) chose head-on feature parity with
`reaatech/mcp-load-test` and rejected repositioning as a bug detector (path C)
because it would throw away load-testing work and narrow the audience.

Since then:

- The only externally confirmed result is still the deadlock class behind
  HKUDS/Vibe-Trading PR #85 (merged 2026-05-09). No load-testing feature has
  produced a comparable finding.
- Parity work grew the workspace to ~49k lines of Rust, including distributed
  SSH load generation and an OTLP exporter, without outside users.
- `reaatech/mcp-load-test` remains at 0 stars (last push 2026-09-28) and has no
  deadlock detection, so parity with it is not what decides adoption.
- Commit `c3f8874` removed distributed load and the OTLP exporter, and
  [ROADMAP.md](../../ROADMAP.md) (2026-10-13) sets the goal as catching
  MCP-specific concurrency bugs that generic load tools do not look for.

## Decision

**Path C, revised.** mcp-loadtest is a CI gate for MCP-specific concurrency
bugs: deadlocks, hangs and divergent responses. Load scenarios stay because
they share the session pool with the detectors and supply the latency
baseline, not as a goal of their own. Feature parity with any load tool is no
longer a goal. New features need a real server or a real bug that motivates
them. Phase 1 of the roadmap (10 real servers) is the decision gate: if it
finds neither a server bug nor a tool defect, pause the project.

## Alternatives considered

| Path | Why not now |
|---|---|
| **A — keep head-on parity (0004)** | Competitor shows no traction, parity adds code without evidence of demand, and the differentiator 0004 itself named as the moat is bug detection. |
| **B — contribute to reaatech** | Same reason as in 0004; their TypeScript architecture does not extend naturally to the detectors. |
| **Drop load scenarios entirely** | Detectors reuse the scenario session pool, and the latency baseline is needed to tell a hang from a slow server. |

## Consequences

- 0004's milestone plan (M1-M7 parity, private until parity) no longer applies.
- Generic load features (multi-machine generation, telemetry exporters,
  protocol-agnostic HTTP load) are non-goals, as listed in the roadmap.
- Success is measured by confirmed findings against real servers, not by
  feature count.
- The audience is smaller, as 0004 warned. The Phase 1 gate tests whether it
  is large enough to continue.
