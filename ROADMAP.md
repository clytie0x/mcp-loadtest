# Roadmap

Status: draft, 2026-10-13. Items are tracked as GitHub issues; this file
states the goal, the order, and the exit criteria.

## Goal

Be the CI gate that catches MCP-specific concurrency bugs (deadlocks, hangs,
divergent responses) that generic load tools such as k6 or Gatling do not
look for. Load scenarios stay because they share the session pool with the
detectors and supply the latency baseline, not as a goal of their own.

## Non-goals

- Features a generic load tool already covers well: multi-machine load
  generation, telemetry exporters, protocol-agnostic HTTP load.
- Breadth for its own sake. A new feature needs a real server or a real bug
  that motivates it.

## Phase 0: Clean base (v0.3.0)

- Merge the removal of distributed load and the OTLP exporter.
- Bring DESIGN.md and ADR 0022 in line with the seven-crate workspace; mark
  distributed mode as removed rather than future work.
- Cut `v0.3.0` through the existing [release procedure](docs/RELEASE.md).

Exit: `scripts/ci-checks.sh` passes on `main` and the `v0.3.0` Release exists
with all platform archives and checksums.

Status: met; `v0.3.0` is published.

## Phase 1: Evidence against real servers

- Run `deadlock-probe`, `race_check`, and a short `sustained` load against at
  least 10 public MCP servers: the official reference servers plus popular
  community servers across Python, TypeScript, and Go.
- Record every run (server, revision, command, verdict) in
  `docs/field-results.md`, including clean passes.
- File each crash, false positive, or confusing message in mcp-loadtest
  itself as an issue.

Exit: 10 servers recorded. Decision gate: if no real server bug is found
and no tool defect turns up, pause the project instead of adding features.

Status: met; see [field-results.md](docs/field-results.md#phase-1-status).

## Phase 2: Turn findings into proof

- For each confirmed server bug, report it upstream with a minimal
  reproduction (as with Vibe-Trading PR #85).
- Add a pinned regression fixture per new bug class under
  `crates/engine/tests/`.
- Link confirmed findings from the README.

Exit: at least one upstream issue or PR acknowledged by its maintainers.

Status: not met. Three open server bugs are recorded in
[field-results.md](docs/field-results.md#phase-1-status); none has been
reported upstream.

## Phase 3: Lower adoption friction

- Verify the GitHub Action end to end on a public sample repository.
- Ship a one-command quickstart that probes a server without a config file.
- Decide crates.io publication: deferred on 2026-10-19 until an external user
  asks for a registry dependency (ADR 0031).

Exit: a new user reaches a first verdict in under five minutes, measured on
a clean machine.

Status: met on 2026-10-19 with `v0.4.1`. The Action demo repository passes
on a pinned tag ([#7](https://github.com/clytie0x/mcp-loadtest/issues/7)).
A clean cloud session went from download to first verdict in about 10
seconds ([#8](https://github.com/clytie0x/mcp-loadtest/issues/8)).
