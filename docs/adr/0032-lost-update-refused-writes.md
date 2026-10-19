# 32. A refused write is not a lost-update error

Date: 2026-10-19
Status: Accepted

## Context

ADR [0027](0027-lost-update-detector.md) counts every failed concurrent
write as an error, and `Report::passed()` fails a `lost_update` run on any
error as an incomplete diagnostic. The 2026-10-19 field run
([field-results.md](../field-results.md)) ran `lost_update` against
obsidian-mcp 2.0.1, which uses optimistic concurrency. It refuses an edit
that conflicts with another with `isError`
(`REVISION_CONFLICT: The note changed after it was read`), and every write
it acknowledged was in the note. That is the correct behavior, yet each run
reported `FAIL (incomplete lost-update check)`. The note said only that the
tool "returned isError", so the reason took a manual reproduction to find.

## Decision

- A concurrent write whose result has `isError` is **refused**. It counts
  toward a new `refused_write_count` on `ScenarioOutcome` and in metrics.json
  v1 (omitted when zero), and not toward `error_count`. A refused write was
  never acknowledged, so it is not expected in the read. It neither fails
  the run nor makes it incomplete.
- The verdict rests on the acknowledged writes, as before: every one must be
  in the final read. If no concurrent write is acknowledged, the check is
  still inconclusive.
- The control write and both reads must still succeed. An `isError` there
  remains an error, because without them the check means nothing.
- Hangs, deadlocks and JSON-RPC or transport errors on a write are still
  errors. Only a tool result the server returned with `isError` is a refusal.
- Notes carry the server's error text: one summary note for refused writes
  with the count and the first refusal's text, and the text inline for an
  `isError` on the control write or a read. Text is collapsed to one line
  and capped at 200 characters.
- The `serve --mcp` `lost_update` tool returns `refused_write_count` next to
  `lost_update_count`. The recorder still records each refusal as a server
  error, so error-rate thresholds see them.

## Alternatives considered

- **Keep failing on any write error.** This fails a server for doing the
  right thing, and it pushes users to drop concurrency until refusals stop,
  which hides the races the check exists to find.
- **Ignore refusals silently.** A server that refuses every write but one
  would pass with no sign of it. The count and the note keep that visible.
- **Treat every write failure as a refusal.** A hang or a dropped
  connection says nothing about whether the write landed, so it cannot
  count as "not acknowledged, not expected".

## Consequences

- A server that refuses most writes because of a bug, rather than a
  deliberate conflict check, now passes if nothing it acknowledged was lost.
  The refusal note and the error rate are where that shows. `lost_update`
  checks durability of acknowledged writes, not availability.
- A run against server-filesystem 2.0.0 now reads `FAIL (N lost updates)`
  instead of `FAIL (N lost updates, incomplete lost-update check)`. The
  losses still fail it.
