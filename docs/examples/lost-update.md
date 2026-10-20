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
  `{worker}` and `{seq}` are substituted too. The `lost-update` subcommand
  can also choose the tools and build the arguments itself (see
  [Letting it choose the tools](#letting-it-choose-the-tools)).
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

## Letting it choose the tools

`--write-tool`, `--write-args` and `--read-tool` are optional. Leave them
out and the check lists the server's tools and chooses from the server's own
annotations ([ADR 0035](../adr/0035-lost-update-picks-tools.md)):

```bash
mcp-loadtest lost-update --pipelined \
  --server "npx -y @modelcontextprotocol/server-memory@2026.8.31" \
  --env MEMORY_FILE_PATH=/tmp/lost-update-probe.jsonl --startup-timeout 60s
```

```text
no --write-tool given; writing with `create_entities` (the server marks it not read-only, not destructive and closed-world) and arguments built from its input schema: {"entities":[{"entityType":"{marker}","name":"{marker}","observations":["{marker}"]}]}. These are the server's own hints; pass --write-tool to choose.
no --read-tool given; reading with `read_graph` (the server marks it read-only and it takes no arguments). Before a loss fails the run, it is confirmed by a second read; pass --read-tool to choose.
Writes are real: point the server at a disposable store.
  status:    FAIL (45 lost updates)
  note:      lost updates: 45 of 50 acknowledged writes to `create_entities` are missing from `read_graph` (first: …)
  note:      lost_update: confirmed by a recheck: after 3 more writes made one at a time, `read_graph` showed all of them and everything it showed before, and still lacks the missing markers
```

The same command against `@modelcontextprotocol/server-memory@2.0.0` picks
the same tools and passes.

- **Write tool:** the first tool the server marks not read-only, with
  `destructiveHint: false` and `openWorldHint: false` both set (MCP treats a
  missing one as `true`), whose arguments can be built. Names that start
  like a creation (`create_`, `add_`, `append`, ...) come first.
- **Write arguments:** built from the tool's input schema. Every required
  string gets `{marker}`, every other required property its smallest valid
  value, and an array one element. A schema it cannot fill with confidence
  (`anyOf`, a string `format` or `pattern`, an array that needs two items)
  is refused with the property's name.
- **Read tool:** the first tool the server marks read-only, not destructive,
  that takes no arguments at all. A read with an optional `limit` or `page`
  is skipped. Names that start like a listing (`read_`, `list_`, ...) come
  first.
- **Recheck:** a read nobody named might list only part of the store. When
  writes are missing from it, the check makes 3 more writes one at a time
  and reads again. The loss counts only if that read shows the new writes
  and everything it showed before. Otherwise the run is reported as
  incomplete, with a note to name the read tool.

Flags you pass are used as given; only the missing parts are chosen. A
server that annotates nothing, such as `@kazuph/mcp-taskmanager` 1.1.1, gets
the list of its tools and why each does not qualify. Naming the two tools is
then enough, and the write arguments are still built:

```bash
mcp-loadtest lost-update --pipelined \
  --server "npx -y @kazuph/mcp-taskmanager@1.1.1" \
  --env TASK_MANAGER_FILE_PATH=/tmp/lost-update-tasks.json --startup-timeout 60s \
  --write-tool request_planning --read-tool list_requests
```

```text
no --write-args given; writing to `request_planning` with arguments built from its input schema: {"originalRequest":"{marker}","tasks":[{"description":"{marker}","title":"{marker}"}]}
Writes are real: point the server at a disposable store.
  status:    FAIL (45 lost updates)
```

`--write-args` needs `--write-tool`, and `--read-args` needs `--read-tool`.

## One process: `--pipelined`

The default mode needs sessions that share a store, and finds races between
server processes. Many servers also lose writes inside one process when a
client sends several tool calls without waiting for each reply, which is what
clients that run tool calls in parallel do. `--pipelined` checks that case:
it uses one session, and each of `--writes-per-worker` rounds sends
`--concurrent` writes back to back, then matches the replies by id. A server
that keeps its state in memory works too.

```bash
mkdir -p /tmp/lu && printf 'END\n' > /tmp/lu/log.txt
mcp-loadtest lost-update --pipelined \
  --server "npx -y @modelcontextprotocol/server-filesystem@2.0.0 /tmp/lu" \
  --write-tool edit_file \
  --write-args '{"path":"/tmp/lu/log.txt","edits":[{"oldText":"END","newText":"{marker}\nEND"}]}' \
  --read-tool read_text_file --read-args '{"path":"/tmp/lu/log.txt"}' \
  --concurrent 5 --writes-per-worker 3
```

```text
  status:    FAIL (12 lost updates)
  requests:  18 total · 18 ok · 0 errors (0.00%)
  note:      lost updates: 12 of 15 acknowledged writes to `edit_file` are missing from `read_text_file` (first: …)
```

Every edit succeeded, and in each round of 5 only one was left in the file.
A server that serializes its writes passes, and one that rejects conflicting
writes passes with the refusals counted (see
[field-results.md](../field-results.md)). Pipelining needs a stdio server:
an HTTP target already shares one process across sessions in the default
mode. If a round's replies do not all arrive within the hang threshold plus
the grace period, the check stops there and reports the missing ones as
deadlocks.

## A repro for the upstream report

Maintainers of the server will want a repro they can run without
mcp-loadtest. Add `--emit-repro` to a `--pipelined` run, and when it finds
lost updates it writes one on the official MCP SDK: a `.mjs` file uses
`@modelcontextprotocol/sdk`, a `.py` file the `mcp` package
(`--repro-sdk node|python` overrides the extension).

```bash
mcp-loadtest lost-update --pipelined \
  --server "npx -y @kazuph/mcp-taskmanager@1.1.1" \
  --env TASK_MANAGER_FILE_PATH=/tmp/tm/tasks.json --startup-timeout 60s \
  --write-tool request_planning \
  --write-args '{"originalRequest":"{marker}","tasks":[{"title":"{marker}","description":"{marker}"}]}' \
  --read-tool list_requests --concurrent 5 --writes-per-worker 2 \
  --emit-repro repro.mjs
```

The script starts the server with the same command, args and env, sends
one round of `--concurrent` writes at once on one session, each with a
unique marker, then calls the read tool once:

```text
$ npm install @modelcontextprotocol/sdk && node repro.mjs
calls: 5 sent, 5 succeeded, 0 isError, 0 failed
read: 1 of 5 acknowledged writes are in the result
LOST: 4 acknowledged writes are missing
```

It exits 1 when an acknowledged write is missing. Only the pipelined
(one-process) case is covered, which is what a client running tool calls in
parallel hits; a loss that needs several server processes has no script.
`run --emit-repro <path>` does the same for a `lost_update` config with
`pipelined = true`. The env is copied into the script, so check it for
secrets before posting it, and trim the store path to something the
maintainers can use.

## Reading the result

| Outcome | Meaning |
|---|---|
| `FAIL (N lost updates)` | N acknowledged writes are missing from the read. The notes list the first few markers, which name the worker and sequence of each lost write. |
| `incomplete lost-update check` | The control step failed (the read did not show a write made on another session, or the control write or a read returned an error), no concurrent write was acknowledged, or a write hung or lost its connection. Check that the read tool returns stored data and that the sessions share a store. With a read the check picked, it also means the recheck could not confirm a loss: name a read tool that returns everything with `--read-tool`. |
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
`refused_write_count`. Add `"pipelined": true` for the one-process mode.

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
