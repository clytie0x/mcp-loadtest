# Field results

Phase 1 of [ROADMAP.md](../ROADMAP.md): run the detectors against real MCP
servers and record every run, including clean passes.

## Run 2026-10-17

### Setup

- Host: Windows 11 (10.0.26300), Node 22.23.2, Python 3.13.15, Go 1.27.1.
- mcp-loadtest: `86fffdc` plus the Unreleased fixes in
  [CHANGELOG.md](../CHANGELOG.md) (uncommitted at run time), release build.
- Every server ran locally over stdio from a pinned package version. Inputs were
  local fixtures: a 2,000-line text file, a 30-commit git repository, a
  30-entity memory graph, a small Python project, a small Go module, and a
  static page on a `127.0.0.1` HTTP server. No run reached an external service.
- Per tool, unless noted: `deadlock_probe` with 20 sessions (hang threshold
  10 s, grace 10 s), `race_check` with 10 sessions, `sustained` for 20 s with
  10 sessions. Every session is its own server process.

### Controls

| Server | Run | Result |
|---|---|---|
| `mock-broken.py` (repo fixture, lazy-init deadlock) | probe, 20 sessions | FAIL as expected: 20/20 deadlocks |
| `@modelcontextprotocol/server-everything` 1.0.0 | probe, race, sustained on `echo`; probe, race on `get-sum` | PASS: 0 deadlocks, 0 hangs, 0 errors, 1,204,681 sustained calls |

### Servers

| # | Server | Lang | Tools exercised | Verdict |
|---|---|---|---|---|
| 1 | `@modelcontextprotocol/server-everything` 1.0.0 | TS | `echo`, `get-sum` | Clean pass (control above) |
| 2 | `@modelcontextprotocol/server-filesystem` 1.0.0 | TS | `read_text_file`, `directory_tree`, `search_files` | Clean pass, 58,304 sustained calls |
| 3 | `@modelcontextprotocol/server-memory` 1.0.0 | TS | `read_graph`, `search_nodes`, `add_observations` | Clean pass. A separate lost-update check (below) also passed |
| 4 | `@playwright/mcp` 0.0.83 (`--headless --isolated`) | TS | `browser_navigate`, `browser_evaluate` | Pass. Inconclusive cold-start latency, see notes |
| 5 | `mcp-server-git` 2026.10.10 | Python | `git_log`, `git_status`, `git_diff` | Clean pass with the fixed tool. Before the fix, the tool itself failed these runs |
| 6 | `mcp-server-fetch` 2026.10.10 (`--allow-private-ips`) | Python | `fetch` (markdown and raw) | Clean pass |
| 7 | `markitdown-mcp` 0.0.1a7 | Python | `convert_to_markdown` (CSV, HTML) | Clean pass with the fixed tool. Before the fix, every run failed in the tool |
| 8 | `serena-agent` 2.0.0 (`--agent-interface tools`, LSP backend) | Python | `find_symbol`, `find_referencing_symbols`, `get_symbols_overview` | Clean pass |
| 9 | `gopls` v0.21.1 (`gopls mcp`) | Go | `go_search`, `go_symbol_references`, `go_package_api` | Clean pass, 20,053 sustained calls |
| 10 | `github/github-mcp-server` | Go | not run | Needs a read-only token; pending |

Overrides: playwright used 5 probe, 5 race and 3 sustained sessions (each
session launches Chrome). Serena used 10/5/5 sessions and a 60 s hang threshold
because its first call starts a language server (about 18 s cold, 1.3 s warm),
plus one extra 20-session probe at the default 10 s threshold, which passed.

### Notes per server

- **server-memory, lost updates.** `race_check` and `sustained` send identical
  arguments, so they cannot show lost writes. A separate script ran 10 and then
  20 server processes on one `MEMORY_FILE_PATH`, each adding unique
  observations. 200/200 and 500/500 (twice) persisted, no corrupt lines. The
  server takes a lock file around writes.
- **playwright, cold start.** Two of four early `browser_evaluate` probes
  recorded 3/5 calls over the 10 s hang threshold (5.5 to 10.7 s). Six later
  repeats all passed at 1.4 to 1.9 s, and no `browser_navigate` probe call
  exceeded 1.3 s. Treated as cold-start latency, not a server bug.
- **mcp-server-git, slow concurrent starts.** With 10 concurrent processes,
  sustained runs were bimodal: about 660 calls at 260 ms p50, or about 10 calls
  at 30 to 39 s each. Traces of a slow run show the servers themselves taking
  over 30 s to answer `server/discover` and the first `tools/call`. No hang
  stayed unanswered. Treated as host-dependent start-up latency, not a
  deadlock.

### Tool defects found

| Defect | Status |
|---|---|
| `[server.env]` and `working_dir` were parsed but never applied to any spawned server. The server-memory runs used the default memory file and failed with "Entity with name e3 not found". | Fixed, with `crates/engine/tests/spawn_env.rs` |
| A `server/discover` reply arriving after the 5 s probe window was read as the `initialize` reply. Late errors failed as `Malformed` (mcp-server-git at 10 sessions). Late successes left the server refusing `initialize` with -32022 (markitdown-mcp, every run). | Fixed, with two tests in `crates/protocol/tests/protocol_2026.rs` |
| Pooled stdio runs print `Peak RSS: 0.0 MB · Avg CPU: 0.0%` as if measured. The executor clears the stats and adds a note, but neither the markdown nor the JSON report shows scenario notes. | Fixed, with `reporters_disclose_unsampled_process_metrics_and_notes` in `crates/output/tests/reporter_snapshots.rs` |
| A run with 3 hangs out of 5 calls reports `Error rate: 60.00%` while its Errors table says "No errors recorded". | Fixed, with `errors_section_lists_hangs_and_deadlocks_counted_by_error_rate` in `crates/output/tests/reporter_snapshots.rs` |
| `race_check` and `sustained` cannot vary arguments per worker, so lost-update races are out of reach (see server-memory note). | Addressed by the `lost_update` scenario in v0.3.0; see the 2026-10-18 run |
| In a trace, a late probe reply is labelled with the method of the request that read it (`initialize`). Replay still pairs frames by id. | Open, cosmetic |

### Gate status

9 of 10 servers recorded. No server bug was confirmed. Tool defects did turn up,
so the roadmap's pause condition is not met. Remaining for the exit criterion:
the github-mcp-server run.

## Run 2026-10-18: `lost_update` on server-memory

### Setup

- Host: Linux container, Node 22.22.0.
- mcp-loadtest `v0.3.0` (`7d1a0a7`), release build.
- `@modelcontextprotocol/server-memory` at two versions, each installed from
  npm: `2026.8.31`, the last dated release, and `1.0.0`, the current `latest`
  tag, published 2026-10-11.
- `lost_update` with 8 sessions × 5 writes. Every session is its own server
  process, and all share one `MEMORY_FILE_PATH` set through `[server.env]`.
  Each write is `create_entities` with one entity named by `{marker}`, and the
  read is `read_graph`. The config is in
  [examples/lost-update.md](examples/lost-update.md).
- `2026.8.31` ran 5 times and `1.0.0` 4 times. After 3 runs of each, the
  entities in the memory file were counted independently of the detector: 41
  expected (40 writes plus the control).

### Results

| Version | Runs | Lost updates (of 40) | Entities in file | p95 latency |
|---|---|---|---|---|
| `2026.8.31` | 5 | 32, 29, 31, 25, 26 | 12, 10, 16 on the 3 counted runs (41 − lost) | 22 to 267 ms |
| `1.0.0` | 4 | 0 every run | 41 on each of the 3 counted runs | 83 ms to 2.1 s |

- **`2026.8.31` loses most concurrent writes.** Each tool call loads the whole
  graph, changes it, and writes the file back, with no lock across processes.
  Concurrent writers overwrite each other, and every call is still
  acknowledged. The file counts match the detector's counts exactly.
- **`1.0.0` is fixed upstream.** It takes a lock file around the
  read-modify-write and writes through a temporary file and a rename. No write
  was lost, consistent with the separate script on 2026-10-17. The lock is
  visible as latency: p95 rose from tens of milliseconds to between 83 ms and
  2.1 s with 8 concurrent writers.
- **Exposure:** a deployment pinned to a dated release (`@2026.8.31` or
  earlier) and shared by more than one client process is affected. Upgrading
  to `1.0.0` fixes it.

### Gate status

This is the first server bug the detectors have confirmed in the field. It
was already fixed upstream before this run, so no report was filed.

## Run 2026-10-18: `lost_update` on mcp-knowledge-graph

### Setup

- Same host and mcp-loadtest build as the server-memory run above.
- `mcp-knowledge-graph` 1.3.2, the current npm release, a fork of
  server-memory that adds named stores. Every process got
  `--memory-path <scratch>/store`, and every call passed
  `location = "global"` so all processes share one file.
- `lost_update` with 8 sessions × 5 writes, through `aim_memory_store`
  (one entity named by `{marker}`) and `aim_memory_read_all`. After each run,
  the entities in `memory.jsonl` were counted independently.

### Results

| Run | Lost updates | Write errors | Entities in file |
|---|---|---|---|
| 1 | 28 of 40 | 0 | 12 |
| 2 | 35 of 40 | 0 | 5 |
| 3 | 35 of 39 | 1: `Unterminated string in JSON at position 94` | 4 |

- **Every run loses most acknowledged writes.** Each call loads the whole
  file, changes it, and writes it back with `fs.writeFile`, with no lock
  across processes. Concurrent writers overwrite each other.
- **Torn reads.** `fs.writeFile` truncates the file before it writes, so a
  concurrent call can read a partial file. In run 3, one call failed to parse
  it. A call that reads the file while it is empty loads an empty graph and
  then saves only its own entity, which also drops data written before the
  concurrent phase: the control entity was gone after every run.
- **Counts agree.** The entities left in each file equal the acknowledged
  concurrent writes minus the lost ones (40 − 28, 40 − 35, 39 − 35). The
  control entity was lost as well. These runs predate the change that
  counts a lost control write, so the lost-update counts above exclude it.
- **Exposure:** any deployment where more than one process uses the same
  store (several agents or editor windows, or one client restarting a
  server while another runs) can lose memories with no error. The upstream
  fix is the one server-memory 1.0.0 made: a lock around the
  read-modify-write and a write to a temporary file followed by a rename.

### Gate status

This is the first open server bug the detectors have found. It has not been
reported upstream yet.

