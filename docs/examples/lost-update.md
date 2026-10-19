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

Without a config file, the `lost-update` subcommand takes the same settings
as flags:

```bash
mcp-loadtest lost-update \
  --server "npx -y @modelcontextprotocol/server-memory@2026.8.31" \
  --env MEMORY_FILE_PATH=/tmp/lost-update-probe.jsonl --startup-timeout 60s \
  --write-tool create_entities \
  --write-args '{"entities":[{"name":"{marker}","entityType":"probe","observations":["w{worker}s{seq}"]}]}' \
  --read-tool read_graph --concurrent 8 --writes-per-worker 5
```

## Reading the result

| Outcome | Meaning |
|---|---|
| `FAIL (N lost updates)` | N acknowledged writes are missing from the read. The notes list the first few markers, which name the worker and sequence of each lost write. |
| `incomplete lost-update check` | The control step failed (the read did not show a write made on another session, or the control write or a read returned an error), no concurrent write was acknowledged, or a write hung or lost its connection. Check that the read tool returns stored data and that the sessions share a store. |
| `refused N concurrent writes` (note) | The server refused N writes with `isError`, for example a conflict check. They were never acknowledged, so they are not expected in the read and do not fail the run. The note quotes the first refusal. |
| `PASS` | Every acknowledged write was read back. A race that needs a narrower window may still exist; raise `concurrent` and `writes_per_worker` to widen it. |

## From an agent

`serve --mcp` exposes the same check as the `lost_update` tool. The config
above becomes one call:

```json
{
  "server_command": "npx -y @modelcontextprotocol/server-memory@2026.8.31",
  "env": { "MEMORY_FILE_PATH": "/tmp/lost-update-probe.jsonl" },
  "startup_timeout_ms": 60000,
  "write_tool": "create_entities",
  "write_args": { "entities": [ { "name": "{marker}", "entityType": "probe", "observations": ["w{worker}s{seq}"] } ] },
  "read_tool": "read_graph",
  "concurrent": 8,
  "writes_per_worker": 5
}
```

The raised `startup_timeout_ms` covers the first `npx` download. The tool
returns `passed`, `failure_reasons`, `lost_update_count`, and
`refused_write_count`.

For a server that is already running, pass `server_url` instead of
`server_command`. Every session then shares that one process, which is
where unlocked in-memory state loses writes:

```json
{
  "server_url": "http://127.0.0.1:3000/mcp",
  "allowed_hosts": ["127.0.0.1"],
  "write_tool": "append",
  "write_args": { "item": "{marker}" },
  "read_tool": "read"
}
```

Loopback and private addresses need `allowed_hosts` (SSRF guard). See
[DESIGN.md](../../DESIGN.md) §21.2 for every argument.
