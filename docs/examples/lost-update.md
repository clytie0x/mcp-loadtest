# Finding lost updates

`lost_update` finds writes that a server acknowledges and then loses, which
is the usual result of an unlocked read-modify-write. `race_check` cannot see
this, because it sends identical calls and compares responses.

This page runs it against `@modelcontextprotocol/server-memory`. Release
`2026.8.31` loses most concurrent writes, and `1.0.0` fixes it with a lock
file. Both results are recorded in
[field-results.md](../field-results.md#run-2026-10-18-lost_update-on-server-memory).

## What the check needs

- **A write tool whose arguments can carry a unique string.** Put `{marker}`
  in a string value of `write_args`. Each call gets a run-unique marker, and
  `{worker}` and `{seq}` are substituted too.
- **A read tool that returns what was stored.** Every acknowledged marker
  must appear in its result.
- **Sessions that share state.** For stdio, each session is its own server
  process, so the processes must share a store: a file, a database, or a
  remote service. Before any concurrent write, the check writes a control
  marker on one session and reads it on another. If the read cannot see it,
  the run is reported as inconclusive, not as lost updates.

Point the check at a disposable store. Every write is real.

## Config

```toml
[server]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-memory@2026.8.31"]

[server.env]
MEMORY_FILE_PATH = "/tmp/lost-update-probe.jsonl"

[scenario]
type = "lost_update"
concurrent = 8
writes_per_worker = 5
write_tool = "create_entities"
write_args = { entities = [ { name = "{marker}", entityType = "probe", observations = ["w{worker}s{seq}"] } ] }
read_tool = "read_graph"
```

```text
$ mcp-loadtest run --config lost-update.toml
mcp-loadtest run summary
  status:    FAIL (26 lost updates)
  scenario:  lost_update
  requests:  43 total · 43 ok · 0 errors (0.00%)
  note:      lost updates: 26 of 40 acknowledged writes to `create_entities` are missing from `read_graph` (first: mlta3000e31w0001s0000x, …)

Error: run failed correctness gate — 26 lost updates; 43/43 successful calls; see report
```

The first `npx` run downloads the package in every server process at once
and can miss the 10 s initialize timeout. Run it again once the package is
cached, or install it first with `npm install -g`.

All 43 calls succeeded, which is the point. The server acknowledged every
write, and 26 of them were gone when the graph was read back. The 43 calls
are the control write and read, 40 concurrent writes, and the final read.

With `@modelcontextprotocol/server-memory@1.0.0`, the same config passes
with 0 lost updates.

## Reading the result

| Outcome | Meaning |
|---|---|
| `FAIL (N lost updates)` | N acknowledged writes are missing from the read. The notes list the first few markers, which name the worker and sequence of each lost write. |
| `incomplete lost-update check` | The control step failed: the read did not show a write made on another session, or no concurrent write was acknowledged. Check that the read tool returns stored data and that the sessions share a store. |
| `PASS` | Every acknowledged write was read back. A race that needs a narrower window may still exist; raise `concurrent` and `writes_per_worker` to widen it. |

## From an agent

`serve --mcp` exposes the same check as the `lost_update` tool, with the same
arguments plus `server_command`. It returns `passed`, `failure_reasons`, and
`lost_update_count`. See [DESIGN.md](../../DESIGN.md) for the tool list.
