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
| Pooled stdio runs print `Peak RSS: 0.0 MB · Avg CPU: 0.0%` as if measured. The executor clears the stats and adds a note, but neither the markdown nor the JSON report shows scenario notes. | Open |
| A run with 3 hangs out of 5 calls reports `Error rate: 60.00%` while its Errors table says "No errors recorded". | Open |
| `race_check` and `sustained` cannot vary arguments per worker, so lost-update races are out of reach (see server-memory note). | Open, feature gap |
| In a trace, a late probe reply is labelled with the method of the request that read it (`initialize`). Replay still pairs frames by id. | Open, cosmetic |

### Gate status

9 of 10 servers recorded. No server bug was confirmed. Tool defects did turn up,
so the roadmap's pause condition is not met. Remaining for the exit criterion:
the github-mcp-server run.
