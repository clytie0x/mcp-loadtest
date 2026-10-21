# 36. Multi-process `lost_update` reads from sessions started after the writes

Date: 2026-10-21
Status: Accepted; amends [0027](0027-lost-update-detector.md)

## Context

ADR 0027 reads the control marker, and later every acknowledged marker, on
the run session. That session's server process starts before any write.

The 2026-10-20 survey ([field-results.md](../field-results.md)) found a
server this misses. mcp-simple-memory 0.4.0 loads its SQLite database into
memory once, at start, and writes its whole copy back on every write. Its
run session never sees a write made by another process. The control read
failed, and the check stopped as inconclusive ("sessions do not share
state"). Meanwhile 12 of 15 acknowledged writes were lost: every writer
overwrote the others. A script that read from a new process after the
writers exited found the loss.

A server like this does share state, through its file, but only with
processes started after the write. That is also how its users meet the
loss: the next client to start sees what the last writer saved.

## Decision

In the multi-process mode, every read comes from a session started after
the writes it checks:

- The **control read** runs on a fresh session, started after the control
  write's session has shut down.
- The **final read**, and a recheck when the read tool was picked (ADR
  [0035](0035-lost-update-picks-tools.md)), run on a fresh session started
  after every writer has shut down.

The run session makes no calls in this mode. The pipelined mode (ADR
[0033](0033-pipelined-lost-update.md)) is unchanged: there, one process is
the point.

## Consequences

- A server that keeps a per-process copy of a shared store is now checked,
  and its lost writes are counted, instead of the run stopping as
  inconclusive. `mock-store.py --mode snapshot` models it.
- A server that keeps data only in process memory is still inconclusive. A
  fresh session sees nothing either, and the note is the same.
- Servers whose processes already share live state see no change: a fresh
  session reads the same store the run session did.
- Each run starts two more server processes (the two readers). The call
  count and the verdict for other servers are unchanged.
