# A concurrency gate for your server's CI

This page adds two checks to a server's pull-request workflow:

- `deadlock-probe` fails the build if a tool call hangs while others are in
  flight, the bug class behind
  [HKUDS/Vibe-Trading PR #85](https://github.com/HKUDS/Vibe-Trading/pull/85).
- `lost_update` fails it if the server acknowledges a write and then loses
  it. Every server in the README's findings table lost writes this way, with
  every call reporting success.

[ci-integration.md](ci-integration.md) covers the Action's inputs and the
latency gate. This page covers the two checks only. Pin the Action and
`version` to a published release.

## 1. Describe a write and a read

Commit a config such as `ci/lost-update.toml`. Pick a tool that stores
something and a tool that returns what was stored, and put `{marker}` in a
string the write stores. This example uses
`@modelcontextprotocol/server-memory`; for your own server, replace the
`[server]` command with the one that starts it.

```toml
[server]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-memory@2.0.0"]
startup_timeout = "60s"

[server.env]
MEMORY_FILE_PATH = "/tmp/mcp-loadtest-probe.jsonl"

[scenario]
type = "lost_update"
pipelined = true
concurrent = 5
writes_per_worker = 3
write_tool = "create_entities"
write_args = { entities = [ { name = "{marker}", entityType = "probe", observations = [] } ] }
read_tool = "read_graph"
```

`pipelined = true` starts one server process and sends each round of 5
writes back to back, the way a client that runs tool calls in parallel does.
That finds races inside one process, which is where the field runs found
most of them ([ADR 0033](../adr/0033-pipelined-lost-update.md)). Without it,
each writer is its own server process, which tests a store shared across
processes. Point the store at a scratch path: every write is real.

## 2. Add the workflow

```yaml
# .github/workflows/concurrency.yml
name: Concurrency gate

on:
    pull_request:

jobs:
    gate:
        runs-on: ubuntu-latest
        steps:
            - uses: actions/checkout@v4

            - uses: actions/setup-node@v4
              with:
                  node-version: "22"

            - name: Deadlock probe
              uses: clytie0x/mcp-loadtest@v0.6.0
              with:
                  version: v0.6.0
                  server: "npx -y @modelcontextprotocol/server-memory@2.0.0"

            - name: Lost updates
              uses: clytie0x/mcp-loadtest@v0.6.0
              with:
                  version: v0.6.0
                  command: run
                  # Required by the Action's inputs; `run` takes the server
                  # from the config.
                  server: "npx -y @modelcontextprotocol/server-memory@2.0.0"
                  args: '["--config","ci/lost-update.toml"]'
```

`deadlock-probe` picks a tool the server marks read-only; pass
`--tool` and `--args` in `args` to choose one. Each step fails the job on a
finding.

Releases after v0.6.0 also accept `command: lost-update`, which takes the
same flags as the `mcp-loadtest lost-update` subcommand and needs no config
file:

```yaml
            - name: Lost updates
              uses: clytie0x/mcp-loadtest@<release after v0.6.0>
              with:
                  version: <same release>
                  command: lost-update
                  server: "npx -y @modelcontextprotocol/server-memory@2.0.0"
                  args: >-
                      ["--pipelined","--concurrent","5","--writes-per-worker","3",
                      "--env","MEMORY_FILE_PATH=/tmp/mcp-loadtest-probe.jsonl",
                      "--write-tool","create_entities",
                      "--write-args","{\"entities\":[{\"name\":\"{marker}\",\"entityType\":\"probe\",\"observations\":[]}]}",
                      "--read-tool","read_graph"]
```

## What the results look like

A server that keeps every write passes:

```text
mcp-loadtest run summary
  status:    PASS
  server:    npx -y @modelcontextprotocol/server-memory@2.0.0
  scenario:  lost_update
  requests:  18 total · 18 ok · 0 errors (0.00%)
```

The same config against `@modelcontextprotocol/server-memory@2026.8.31`,
the release before the fix, fails even though every call succeeded:

```text
  status:    FAIL (12 lost updates)
  note:      lost updates: 12 of 15 acknowledged writes to `create_entities` are missing from `read_graph` (first: …)
Error: run failed correctness gate — 12 lost updates; 18/18 successful calls; see report
```

A server that refuses a conflicting write with an error, instead of losing
it, passes: refused writes are counted and reported, not treated as lost
([ADR 0032](../adr/0032-lost-update-refused-writes.md)).

If a check fails on a server that is behaving correctly, or the message does
not tell you what to fix, please
[open an issue](https://github.com/clytie0x/mcp-loadtest/issues).
