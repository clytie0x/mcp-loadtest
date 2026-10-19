# Field results

Phase 1 of [ROADMAP.md](../ROADMAP.md): run the detectors against real MCP
servers and record every run, including clean passes.

## Phase 1 status

10 servers have had the roadmap's full set (`deadlock-probe`, `race_check`
and a short `sustained` load): the 9 in the 2026-10-17 table and
`mcp-server-time` 2026.10.19. This meets the Phase 1 exit in the roadmap.
`mcp-knowledge-graph` 1.3.2 and `mcp-server-sqlite` 2025.4.25 were run with
`lost_update` only, so they do not count toward it. github-mcp-server is still
not run; it needs a read-only token.

Five open server bugs have been found, all lost updates:
mcp-knowledge-graph 1.3.2, `@modelcontextprotocol/server-filesystem` 2.0.0,
`@kazuph/mcp-taskmanager` 1.1.1, mcp-shrimp-task-manager 1.0.21, and
basic-memory 0.23.2. server-filesystem and mcp-taskmanager are from the
2026-10-19 file-backed run, the last two from the pipelined run that
followed. On 2026-10-19 the project owner decided not to report the
mcp-knowledge-graph bug upstream. None of the five has been reported, so
the Phase 2 exit is not met.

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

## Run 2026-10-18: `lost_update` on mcp-server-sqlite

### Setup

- Linux container, Python 3.13, uv. mcp-loadtest from `main` plus the fixes
  listed under "Tool defects found", debug build.
- `mcp-server-sqlite` 2025.4.25, the last release, run as
  `uvx --exclude-newer 2025-05-01 mcp-server-sqlite@2025.4.25 --db-path <scratch>/t.db`.
  Without `--exclude-newer`, uv resolves today's `mcp` SDK, which the server
  no longer starts against.
- One table `notes(body text)` created beforehand. `lost_update` with 10
  sessions × 5 writes through `write_query`
  (`INSERT INTO notes(body) VALUES ('{marker}')`) and `read_query`
  (`SELECT body FROM notes`). Every session is its own server process on the
  same database file.

### Results

| Run | Protocol | Calls | Lost updates | Verdict |
|---|---|---|---|---|
| 1 | pinned `2025-03-26` | 53/53 | 0 | PASS |
| 2 | pinned `2025-03-26`, after the launcher fix | 53/53 | 0 | PASS |
| 3 | `auto`, after the discovery fix | 53/53 | 0 | PASS |

- **No lost updates.** Each call opens its own connection and commits one
  `INSERT`, and SQLite serializes writers with its file lock. The detector
  reports a correct store as correct.

### Tool defects found

- **Servers behind a launcher outlived the run.** The server ignores stdin
  EOF, so `uvx` stays alive and every shutdown takes the forced-kill path,
  which killed only `uvx`. Run 1 left 12 server processes running, one per
  session, and took 38 s. Fixed in clytie0x/mcp-loadtest#22: run 2 left none
  and took 31 s.
- **`auto` mode could not reach the server.** Its `mcp` SDK exits on the
  unknown `server/discover` probe, so every session failed with "connection
  closed" until the version was pinned. Fixed in clytie0x/mcp-loadtest#23
  (ADR 0029): run 3 restarts each server once with the legacy handshake and
  warns once.

### Gate status

A clean pass for the server. Both tool defects are fixed on `main`.

## Run 2026-10-19: `lost_update` rerun on mcp-knowledge-graph

### Setup

- Linux container, Node 22.22.0. mcp-loadtest from `main` at `47edb55`,
  release build. This build counts a lost control write; the 2026-10-18
  knowledge-graph run predates that.
- `mcp-knowledge-graph` 1.3.2, still the latest npm release. Same setup as
  the 2026-10-18 run: every process got `--memory-path <scratch>/store`, and
  every call passed `location = "global"`.
- `lost_update` with 8 sessions × 5 writes, through `aim_memory_store`
  (one entity named by `{marker}`) and `aim_memory_read_all`. Each run started
  from a fresh store. After each run, the entities in `memory.jsonl` were
  counted independently.

### Results

| Run | Lost updates | Control write | Write errors | Entities in file |
|---|---|---|---|---|
| 1 | 38 of 40 | lost | 0 | 2 |
| 2 | 34 of 36 | lost | 4: `Unexpected non-whitespace character after JSON at position 93` | 2 |
| 3 | 38 of 40 | lost | 0 | 2 |

- **Counts agree.** The tool's totals were 39, 35 and 39 lost updates: the
  missing writes plus the control. The entities left in each file equal the
  acknowledged writes minus the lost ones (40 − 38, 36 − 34, 40 − 38).
- **One process is enough.** A separate script started one server process
  and sent `aim_memory_store` calls in parallel without waiting for replies.
  With 2, 5 and 10 parallel calls, 3 runs each, every call returned success
  and exactly 1 entity was left each time. The same calls sent one after
  another all survive. Every call reads the file before any of them writes,
  so the last writer wins. A single client that issues parallel tool calls
  loses writes; no second process is needed.
- **Across processes, earlier data is lost too.** `fs.writeFile` truncates
  the file before it writes, so the cross-process case also loses data
  written before the concurrent phase. The control was lost in every run.
- **Upstream `main` is the same.** Upstream `main` at `9f6268c` (version
  1.4.0, not yet on npm) has the same `loadGraph` and `saveGraph` code. The
  script with 3 parallel calls left 1 entity in 3 of 3 runs against a build
  of it.

### Gate status

This confirms the 2026-10-18 result. On 2026-10-19 the project owner decided
not to report it upstream.

## Run 2026-10-19: mcp-server-time

### Setup

- Linux container, Python 3.13.16, uv 0.11.32. mcp-loadtest `v0.4.0`
  candidate (`14a53d2`), release build.
- `mcp-server-time` 2026.10.19, the current PyPI release, on `mcp` 2.3.0. Run
  as `uvx mcp-server-time@2026.10.19 --local-timezone UTC`. No run reached an
  external service.
- The same settings as the 2026-10-17 run: `deadlock_probe` with 20 sessions
  (hang threshold 10 s, grace 10 s), `race_check` with 10 sessions, and
  `sustained` for 20 s with 10 sessions. Every session is its own server
  process.

### Results

| Run | Tool | Result |
|---|---|---|
| probe, 20 sessions | `get_current_time` (`timezone = "UTC"`) | PASS: 0 deadlocks, 0 hangs, 20/20 ok |
| probe, 20 sessions | `convert_time` (12:00 UTC to Asia/Bangkok) | PASS: 0 deadlocks, 0 hangs, 20/20 ok |
| race, 10 sessions | `convert_time` | PASS: 10/10 ok, no divergence |
| sustained, 20 s, 10 sessions | both tools, equal weight | PASS: 74,562 calls, 0 errors, p50 2.5 ms, p99 5.7 ms |

- **No race check on `get_current_time`.** Its answer changes every second,
  so calls that straddle a second boundary would differ without any bug.
  `convert_time` with a fixed time gives the same answer to every call on the
  same UTC date; the server adds today's date to its result, so a race check
  that straddles midnight UTC would differ too. This one ran at about 10:30
  UTC.
- **The quickstart stops here.** Both tools are marked read-only but take
  required arguments, so `deadlock-probe` without `--tool` lists them and
  asks for `--tool` and `--args`.
- **No processes left behind.** No `mcp-server-time` process was running
  after the runs, although every session started through `uvx`.

### Gate status

A clean pass, and the tenth server with the full set.

## Run 2026-10-19: `lost_update` on file-backed servers

### Setup

- Linux container, Node 22.22.0. mcp-loadtest `v0.4.1` release binary.
- Five servers that keep state in a file or a database, each installed from
  npm at the version below and run against a scratch store:
  `@modelcontextprotocol/server-filesystem` 2.0.0 (also 1.0.0 and 2026.8.31
  for comparison), `@kazuph/mcp-taskmanager` 1.1.1, `obsidian-mcp` 2.0.1,
  `mcp-memory-keeper` 0.14.1, and `@iachilles/memento` 0.7.0.
- `lost_update` with 8 sessions × 5 writes, 3 runs per server, fresh store
  each run. Every session is its own server process sharing the store. After
  each run, the store was counted independently.
- A separate script started one server process and sent N write calls in
  parallel without waiting for replies, then read the store once: N = 2, 5,
  10 with 3 runs each for server-filesystem 2.0.0 and mcp-taskmanager; one
  run each for N = 2, 5, 10 on server-filesystem 1.0.0 and N = 2, 10 on
  2026.8.31.

| Server | Write → read | Lost updates (3 runs) | Write errors | One process, N parallel |
|---|---|---|---|---|
| server-filesystem 2.0.0 | `edit_file` (replace `END` with `{marker}\nEND` in one file) → `read_text_file` | 18 of 30, 16 of 30, 21 of 31 | 10, 10, 9 | 1 of N kept |
| server-filesystem 1.0.0 | same | 15 of 25, 21 of 32, 19 of 29 | 15, 8, 11 | 1 of N kept |
| server-filesystem 2026.8.31 | same | 30 of 40, 29 of 40, 31 of 40 | 0 | 1 of N kept |
| mcp-taskmanager 1.1.1 | `request_planning` (`originalRequest = {marker}`) → `list_requests` | 39, 37 and 34 of 40, control lost every run | 0 | 1 of N kept |
| obsidian-mcp 2.0.1 | `obsidian_edit_note` (append to one note) → `obsidian_read_note` | 0 | 16, 9, 14 (`REVISION_CONFLICT`) | 1 acknowledged, rest refused |
| mcp-memory-keeper 0.14.1 | `context_save` → `context_search_all` | 0 (PASS) | 0 | not run |
| memento 0.7.0 | `create_entities` → `read_graph` | not run | control write: `fetch failed` | not run |

### Results

- **server-filesystem loses parallel edits to one file.** `edit_file` reads
  the file, applies the edits, and writes it back with no lock. With one
  server process and 2, 5 or 10 parallel `edit_file` calls on the same file,
  every call returned success with a diff, and only one edit was in the file,
  in all 9 runs on 2.0.0 and in every run on 1.0.0 and 2026.8.31. Upstream
  `main` at `51b6382` has the same code. An agent that issues parallel tool calls
  against one file loses edits with no error.
- **Node 24 behaves the same.** server-filesystem 2.0.0 declares
  `node >= 24`, and the runs above used Node 22.22.0. Repeated on Node
  24.22.0: one process kept 1 of 2, 5 and 10 parallel edits; across
  processes, 3 runs lost 17, 13 and 15 edits with 9, 10 and 11 write errors,
  and 2 of the 3 files ended torn.
- **Since 1.0.0 it can also tear the file.** 1.0.0 replaced the temp-file
  rename with an in-place `truncate(0)` and write. Across processes, a reader
  can now see the file empty or half-written. That is where the write errors
  come from: `Could not find exact match for edit: END`. It also leaves torn
  content. In 2 of 3 extra 2.0.0 runs, and 2 of 3 1.0.0 runs, the file ended
  with a partial marker line or a second `END` line. 2026.8.31 lost as many
  writes but never errored or tore the file.
- **mcp-taskmanager loses requests, and the request IDs it returns are
  wrong.** Every tool loads `tasks.json`, changes it, and saves it with no
  lock. Across processes the control request was lost in every run. The
  requests left in the file match the tool's count (1, 3 and 6). With one
  process and N parallel `request_planning` calls, every call succeeded,
  and the file kept exactly one request, `req-N`. Upstream `main` at
  `e9f09ab` (1.1.1) is the same.
- **obsidian-mcp handles it correctly.** Conflicting appends are refused
  with `REVISION_CONFLICT: The note changed after it was read`, and every
  acknowledged write was in the note (24, 31 and 26 of them, plus the
  control). v0.4.1 reported this as `FAIL (incomplete lost-update check)`;
  see the defects below.
- **mcp-memory-keeper passes.** It stores items in SQLite; all 41 rows
  (40 writes and the control) were in `context.db` after the last run.
- **memento was not run.** Its first write downloads an embedding model,
  which the container's network policy blocks.

### Tool defects found

| Defect | Status |
|---|---|
| `lost_update` reports any write error as `incomplete` and fails the run, even when the server refused the write and every acknowledged write was read back (obsidian-mcp). The note says only `returned isError`, without the server's error text. | Fixed in [#33](https://github.com/clytie0x/mcp-loadtest/pull/33) (ADR 0032): refused writes count toward `refused_write_count`, and obsidian-mcp now passes with the refusal text in the notes |

### Gate status

Two new open server bugs, in server-filesystem and mcp-taskmanager. Neither
has been reported upstream yet.

## Run 2026-10-19: pipelined `lost_update` on one process

### Setup

- Linux container, Node 22.22.0. mcp-loadtest built from the branch that
  adds `--pipelined` (ADR 0033), debug build.
- The same packages and write/read pairs as the file-backed run above, each
  started once: `lost-update --pipelined --concurrent 5 --writes-per-worker 3`,
  so 3 rounds of 5 writes sent back to back on one session. Fresh store each
  run.

### Results

| Server | Runs | Result |
|---|---|---|
| server-filesystem 2.0.0 (`edit_file`) | 3 | FAIL every run: 12 of 15 acknowledged writes lost, 18/18 calls ok |
| mcp-taskmanager 1.1.1 (`request_planning`) | 3 | FAIL every run: 12 of 15 acknowledged writes lost, 18/18 calls ok |
| mcp-memory-keeper 0.14.1 (`context_save`) | 2 | PASS: every write read back |
| obsidian-mcp 2.0.1 (`obsidian_edit_note`) | 2 | PASS: 12 of 15 writes refused with `REVISION_CONFLICT`, every acknowledged one read back |

- **The tool now finds these directly.** One write of each round of 5
  survived, the same 1-of-N the separate script showed in the file-backed
  run. That run needed the script because the default mode gives each
  writer its own process.
- **The servers that passed before still pass.** memory-keeper serializes
  writes through SQLite; obsidian-mcp refuses the conflicting ones.

### Gate status

No new server bug; this confirms the two open ones with the tool itself.

## Run 2026-10-19: pipelined `lost_update` on more servers

### Setup

- Linux container, Node 22.22.0. mcp-loadtest from `main` at `bd2e2bd`
  (pipelined mode), debug build; the shrimp runs used the build with the
  stdio fix below.
- `lost-update --pipelined --concurrent 5 --writes-per-worker 3` unless
  noted, fresh store each run. server-memory also ran in the default mode
  with 8 sessions × 5 writes.

### Results

| Server | Write → read | Mode | Result |
|---|---|---|---|
| `@modelcontextprotocol/server-memory` 2.0.0 | `create_entities` → `read_graph` | pipelined, 3 runs | PASS |
| `@modelcontextprotocol/server-memory` 2.0.0 | same | 8 processes, 2 runs | PASS |
| mcp-knowledge-graph 1.3.2 | `aim_memory_store` → `aim_memory_read_all` | pipelined, 3 runs | FAIL: 12, 12 and 11 of 15 lost |
| mcp-shrimp-task-manager 1.0.21 | `split_tasks` (`updateMode: append`, one task named by `{marker}`) → `list_tasks` (`status: all`) | pipelined, 3 runs | FAIL: 12 of 15 lost every run; `tasks.json` held 4 tasks |
| basic-memory 0.23.2 (PyPI, `uvx --prerelease=allow`) | `edit_note` (`operation: append` to one note) → `read_note` | pipelined, 3 runs | FAIL: 6, 9 and 7 of 15 lost, 18/18 calls ok |

- **server-memory 2.0.0 keeps every write** in both modes: the in-process
  mutex and the lock file from the 1.0.0 fix are still in place.
- **mcp-knowledge-graph is caught without the separate script.** The
  pipelined mode reproduces the 2026-10-19 rerun's one-process finding.
- **mcp-shrimp-task-manager loses writes inside one process.** Every tool
  calls `readTasks()`, changes the list, and `writeTasks()` with
  `fs.writeFile`, with no lock. Of each round of 5 `split_tasks` calls that
  all returned success, one task was saved: the file held the control
  plus 3. Upstream `main` at `8a3be6b` (also 1.0.21) has the same
  `readTasks`/`writeTasks`.
- **basic-memory loses appends to one note.** `edit_entity_with_content`
  reads the note file, builds the new content and writes it back, with
  awaits in between and no lock per note. A separate script with the
  official Python SDK (mcp 2.2.0) sent 5 appends at once; all succeeded and
  3, 4 and 2 lines were kept in three runs. Upstream `main` at `ad9158a` has
  the same path. Installing 0.23.2 with `uvx` needs `--prerelease=allow`,
  because it depends on a fastmcp pre-release.

### Tool defects found

| Defect | Status |
|---|---|
| The stdio transport read a request from the server (`roots/list` from mcp-shrimp-task-manager) as the response to its own call and failed the session. Shrimp holds `tools/list` until `roots/list` is answered, so no scenario could run. | Fixed, [#40](https://github.com/clytie0x/mcp-loadtest/issues/40), with `crates/engine/tests/server_requests.rs` |

### Gate status

Two new open server bugs, in mcp-shrimp-task-manager and basic-memory; not
reported upstream.
