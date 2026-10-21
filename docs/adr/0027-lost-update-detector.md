# 27. Lost-update detector with per-write markers

Date: 2026-10-18
Status: Accepted; the handling of refused writes is amended by
[0032](0032-lost-update-refused-writes.md), and the session that reads by
[0036](0036-lost-update-reads-from-fresh-sessions.md)

## Context

`race_check` releases N identical calls and compares the responses. It
finds nondeterminism, but it cannot see the most common concurrency bug in
stateful MCP tools: an unlocked read-modify-write in which one writer
overwrites another. Every write is acknowledged, every response looks
alike, and data is silently lost. [field-results.md](../field-results.md)
recorded this gap: the detectors had no way to give each worker different
arguments or to check state afterwards.

Agents are the main callers (ADR [0024](0024-reposition-as-mcp-bug-detector.md)),
so the result must be a verdict they can branch on, not a number they have
to interpret. It also must not report a bug where the target simply keeps
per-session state.

## Decision

Add a `lost_update` scenario, a CLI/config entry, and a `serve --mcp` tool.

- **Unique markers.** Each write carries a run-unique, fixed-width marker
  (`mlt<nonce>w<worker>s<seq>x`) substituted for `{marker}` in string
  values of `write_args`. `{worker}` and `{seq}` are also substituted.
  Fixed width and the delimiters keep one marker from being a substring of
  another. A `write_args` without `{marker}` is rejected before any process
  is spawned.
- **Control first, across sessions.** One write on a fresh session, then
  one read on the run session, must show the control marker. That proves the
  read observes writes and that sessions share state. If it fails, the check
  is inconclusive and stops before any concurrent write, instead of yielding
  a false pass or a false finding (for example, a stdio server holding data
  in process memory, where each session is its own process).
- **Concurrent writes.** `concurrent` independent sessions, released
  through the pool's start gate (ADR [0017](0017-session-pool-in-scenario.md)),
  each send `writes_per_worker` writes. Only acknowledged writes are
  expected to persist; failed calls are counted as errors, not losses.
- **One final read.** Every acknowledged marker missing from the read's
  serialized result counts toward `lost_update_count`. A non-zero count
  fails `Report::passed()`, like `divergence_count`.
- **Inconclusive, not lost.** A failed control, or a run in which no
  concurrent write was acknowledged, is reported as an incomplete diagnostic
  with a note and counts no losses. Once the control has passed, a final
  read that shows none of the markers counts all of them as lost: a store
  that interleaved writes into unreadable data loses everything, and that
  must fail the run rather than read as "state not shared".

`lost_update_count` joins `ScenarioOutcome`, the metrics.json v1 wire
format (omitted when zero, so v1 consumers see no change), the Prometheus,
JUnit, and failure-reason outputs, and the config schema. The serve tools
also gain an optional `report_dir` (default `./runs`) and return the
`run_dir` they wrote, so an agent can find the artifacts.

## Alternatives considered

- **Per-worker argument lists in `race_check`.** This lets callers vary
  inputs but still compares responses, not state, so it cannot see an
  overwrite.
- **Inferring "state not shared" from a final read with no concurrent
  markers.** An early version did this. A racy store whose interleaved
  writes leave unparseable data produces exactly that read, so a total loss
  was reported as inconclusive. The cross-session control decides this
  before any concurrency instead.
- **Counter semantics (increment N times, expect N).** This only fits
  servers that expose a counter. An append/read pair with opaque markers
  works with any tool that stores text and returns it.
- **Comparing against a caller-supplied expected state.** This puts
  knowledge of the target's data model on the agent. Markers make the
  expectation self-contained.

## Consequences

- The read tool must return what it stored somewhere in its result. A read
  that summarizes (counts, hashes) is caught by the control step and
  rejected, not misreported.
- Writes land in the target's real store. Callers point the tool at a
  disposable instance, as they would for `sustained_load`.
- Detection is probabilistic: a race that needs a narrower window than the
  start gate gives may pass. Raising `concurrent` and `writes_per_worker`
  widens the window. A pass means "not observed", not "absent".
