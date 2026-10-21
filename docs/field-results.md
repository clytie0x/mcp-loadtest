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

Six server bugs have been found, all lost updates:
mcp-knowledge-graph 1.3.2, `@modelcontextprotocol/server-filesystem` 2.0.0,
`@kazuph/mcp-taskmanager` 1.1.1, mcp-shrimp-task-manager 1.0.21,
basic-memory 0.23.2, and `@danielmarbach/mnemonic-mcp` 1.0.0.
server-filesystem and mcp-taskmanager are from the 2026-10-19 file-backed
run, shrimp and basic-memory from the pipelined run that followed, and
mnemonic from the run after that. basic-memory has since been fixed on
upstream `main` (not yet released); the other five are still open upstream.
On 2026-10-19 the project owner decided not to report the
mcp-knowledge-graph bug upstream, which leaves four to report. None has
been reported yet, so the Phase 2 exit is not met; see the
[upstream check](#upstream-check-2026-10-19).

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

## Run 2026-10-19: mnemonic-mcp

### Setup

- Linux container, Node 22.22.0. mcp-loadtest from `main` at `0517754`,
  debug build.
- `@danielmarbach/mnemonic-mcp` 1.0.0 from npm, with `VAULT_PATH` and
  `HOME` pointing at a fresh directory per run. The server commits every
  change to a git repository in the vault, so `HOME` held a `.gitconfig`
  with a user name and email; without one every commit failed with
  "Author identity unknown" and the vault stayed uncommitted. No embedding
  service was running, so embeddings were skipped.
- `lost-update --concurrent 5 --writes-per-worker 3`, pipelined and
  default mode, fresh vault each run.

### Results

| Write → read | Mode | Result |
|---|---|---|
| `remember` (`title: {marker}`) → `list` (`limit: 200`) | pipelined, 3 runs | PASS, 16 notes and 17 commits each |
| same | 5 processes, 3 runs | PASS, 16 notes and 17 commits each |
| `relate` (one note to N others, `bidirectional: false`), N sent at once | script, 4 runs (N = 2, 5, 10, 10) | 1 of N relations kept every run, N/N calls ok |
| same, one call at a time | script, N = 10 | 10 of 10 kept |

- **`remember` keeps every write.** Each call creates its own note file, and
  `GitOps` serializes `add` and `commit` behind a mutation lock with retries
  on `index.lock`, so every note was committed.
- **`relate` loses relations added to the same note at once.** The tool
  reads both notes, appends the new entry to the note's `relatedTo`, and
  writes the note back. The git lock covers only the commit, not that
  read-modify-write, so the last write wins. After 10 concurrent calls the
  note listed one relation, and the git log had one `relate` commit, for a
  different relation than the one in the file. Upstream `main` at `37b59db`
  (the 1.0.0 release) has the same code in `src/tools/relate.ts`.
- `lost_update` itself cannot express the `relate` case: each write must
  name an existing note id, and mnemonic's ids carry a hash that is only
  known after `remember` returns. A script with the same pipelining found
  it.

### Gate status

One new open server bug, in mnemonic-mcp; not reported upstream.

## Upstream check 2026-10-19

Before reporting the open bugs upstream ([#36](https://github.com/clytie0x/mcp-loadtest/issues/36)),
each draft's repro was run 3 times with an official MCP SDK
(`@modelcontextprotocol/sdk` 1.32.1 on Node 22.22.0, `mcp` 2.3.0 on Python
3.13), against the published release and, where `main` has moved past it,
a build of upstream `main`. Each repo's issues and PRs were searched for an
existing report.

| Server | Release | Upstream `main` | Existing report | Status |
|---|---|---|---|---|
| server-filesystem 2.0.0 | 3/3 lost | 3/3 lost (`51b6382`) | none | open, not filed yet |
| mcp-taskmanager 1.1.1 | 3/3 lost | same commit (`e9f09ab`) | none | open, not filed yet |
| mcp-shrimp-task-manager 1.0.21 | 3/3 lost | 3/3 lost (`8a3be6b`) | none | open, not filed yet |
| mnemonic-mcp 1.0.0 | 3/3 lost | 3/3 lost (`37b59db`) | none | open, not filed yet |
| basic-memory 0.23.2 | 3/3 lost | **fixed** (`ad9158a`) | none | fixed upstream, unreleased |

- **basic-memory `main` no longer loses appends silently.** The race is
  still there, but every append that loses it now returns an error, `The
  note was modified concurrently. Reload the latest content and retry.`, and
  kept plus refused added up to 5 in all three runs (3+2, 4+1, 2+3). This
  matches basic-memory #1684 and #1693. `lost_update` would count those as
  refused writes and pass. The fix is not in a release yet; 0.23.2 on PyPI
  still loses appends with no error. It is no longer reported upstream.
- **mcp-taskmanager now has a pinned regression test**,
  `crates/engine/tests/taskmanager_regression.rs`, which runs `lost_update`
  against `@kazuph/mcp-taskmanager@1.1.1` in both modes and expects lost
  updates.

### Gate status

Four open server bugs still need upstream reports: server-filesystem,
mcp-taskmanager, mcp-shrimp-task-manager and mnemonic-mcp. The issue text is
ready in #36; filing is pending.

## Run 2026-10-20: field survey of servers that keep state

### Setup

- Linux container, Node 22.22.0, Python 3.13.16 and uv 0.11.32 (uvx ran
  the PyPI servers on a uv-managed Python 3.14). mcp-loadtest `v0.6.0` at
  `6a934fc`, release build.
- Candidates came from the official MCP Registry (every page of
  `/v0/servers?version=latest`, 15,545 npm and PyPI packages with a stdio
  transport), filtered by keyword (memory, task, todo, notes, kanban, sqlite,
  knowledge graph, journal) and by required environment variables, plus
  `npm search`. They were ranked by npm weekly downloads and pypistats
  last-week downloads on 2026-10-20 (the "Downloads" column below). Servers
  already in this file were not re-run.
- Every server was installed at the pinned version in the table, with
  `npx -y pkg@version` or `uvx pkg==version`. One server,
  `@simonblanco/kanban-mcp`, keeps its database inside its package
  directory, so it ran from a private `npm install` under `/tmp` instead of
  the shared npx cache.
- Every run got a fresh scratch directory under `/tmp`: `HOME` pointed into
  it, the server's working directory was its store directory, and the store
  path was set through the server's own variable or flag where it has one.
  After each server, the files were checked to be inside the scratch
  directory.
- Per server: `lost-update --pipelined --concurrent 5 --writes-per-worker 3`
  3 times, then the default mode (5 processes sharing the store,
  `--concurrent 5 --writes-per-worker 3`) once, then `deadlock-probe
  --concurrent 5` once on the read tool with the read arguments, on a fresh
  store. `--startup-timeout 60s` for `lost-update`. `deadlock-probe` has no
  `--env` or `--startup-timeout`, so every server command was wrapped in
  `env --chdir=<store> HOME=<scratch> ...`, and packages were cached by a
  tool listing first.
- The write and read tools were picked by hand from each tool list. For
  every FAIL, the store was counted independently and the published
  package's write path was read (npm tarball or uv wheel). Upstream `main`
  was not checked.

### Results

37 servers were tested: 26 from npm and 11 from PyPI. "Pipelined" is the 3
runs of 15 writes on one process; "Processes" is the one run with 5
processes on one store. Counts are lost of acknowledged writes; refused
writes (the server answered `isError`) are listed where there were any.

| # | Server | Downloads/wk | Write → read | Pipelined (3 runs) | Processes | Probe | Verdict |
|---|---|---|---|---|---|---|---|
| 1 | `@wonderwhy-er/desktop-commander` 0.3.0 | 195,946 | `edit_block` (replace `END` with `{marker}\nEND` in one file) → `read_file` | FAIL: 12, 12, 12 of 15 | PASS | PASS | **Lost updates** |
| 2 | `@hasna/todos` 0.17.4 (`todos-mcp`, `HASNA_TODOS_LOCAL=1`) | 23,059 | `create_task` → `list_tasks` (`limit: 500`) | PASS ×3 | PASS | PASS | Clean pass |
| 3 | `@hasna/mementos` 0.17.10 (`mementos-mcp --stdio`, `HASNA_MEMENTOS_LOCAL=1`) | 13,648 | `memory_save` → `memory_list` (`limit: 500`) | PASS ×3 | PASS | PASS | Clean pass |
| 4 | `mcp-server-motherduck` 1.1.0 (`--db-path`, `--read-write`) | 5,096 | `execute_query` (`INSERT`) → `execute_query` (`SELECT`) | PASS ×3 | 15 refused: DuckDB file lock | 4 errors: same lock | Pass; one process per file by design |
| 5 | `anneal-memory` 0.1.1 | 5,035 | `record` → `recall` (`limit: 100`) | PASS ×3 | PASS | PASS | Clean pass |
| 6 | `kindex` 0.49.0 (`kin-mcp`, `kindex[mcp]`) | 2,146 | `task_add` → `task_list` (`limit: 500`) | PASS ×3 | PASS | PASS | Clean pass |
| 7 | `graph-mind-memory` 0.9.20 | 2,086 | `brain_remember` → `brain_recall` (`query: probe`, `limit: 50`) | PASS ×3 | PASS | PASS | Clean pass |
| 8 | `inspeximus` 3.21.0 (`inspeximus[mcp]`, `inspeximus mcp`) | 1,730 | `remember` → `recall` (`k: 100`) | PASS ×3 | PASS | PASS | Clean pass |
| 9 | `cpersona` 2.5.3 | 1,127 | `store` → `list_memories` | PASS ×3 | PASS | PASS | Clean pass |
| 10 | `@itseasy21/mcp-knowledge-graph` 1.0.7 | 916 | `create_entities` → `read_graph` | FAIL: 12, 12, 12 of 15 | FAIL: incomplete, store file torn | PASS | **Lost updates, corrupted store** |
| 11 | `mcp-sqlite` 1.0.9 | 772 | `create_record` → `read_records` | PASS ×3 | PASS | PASS | Clean pass |
| 12 | `compound-memory` 0.5.9 | 686 | `memory_write` → `memory_search` (`top_k: 100`) | PASS ×3 | FAIL: 8 of 15 (repeated: 8) | PASS | **Lost index updates across processes** |
| 13 | `@vibeflow-tools/cli` 0.21.1 (`vf mcp --project`) | 676 | `create_task` → `list_tasks` (`limit: 0`) | PASS ×3 | PASS | PASS | Clean pass |
| 14 | `sessionmem` 1.8.0 (`run`) | 597 | `storeMemory` → `listMemories` | PASS ×3 | PASS | PASS | Clean pass |
| 15 | `@zensation/mcp` 0.3.0 | 522 | `zenbrain_store` → `zenbrain_recall` (`limit: 100`) | PASS ×3 | PASS | PASS | Clean pass |
| 16 | `memory-fabric` 1.4.4 (`memory-fabric[mcp]`) | 404 | `write_local_memory_tool` (append to one section) → `read_section_tool` | PASS ×3 | PASS | PASS | Clean pass |
| 17 | `actari-mcp` 0.19.1 | 400 | `record_artifact` → `list_artifacts` | PASS ×3 | PASS, 11 refused (`database is locked`) | PASS | Clean pass |
| 18 | `engram-chateau` 1.10.0 (`engram-mcp`) | 390 | `engram_diary_write` → `engram_diary_read` | PASS ×3 | PASS | PASS | Clean pass |
| 19 | `memory-lean` 1.5.0 | 321 | `create_entities` → `read_graph` | PASS ×3 | PASS | PASS | Clean pass |
| 20 | `@ai-devkit/memory` 0.20.3 | 262 | `memory_storeKnowledge` → `memory_searchKnowledge` | PASS ×3 | PASS, 6 refused (`database is locked`) | 1 error | Pass; errors on concurrent first start |
| 21 | `@jungjaehoon/mama-server` 2.6.0 | 221 | `save` (decision) → `search` (`limit: 100`) | PASS ×3 | Inconclusive: 5 hangs on cold model load | PASS | Pass in one process |
| 22 | `skillmem` 0.12.1 | 181 | `mem_write` → `mem_list` | PASS ×3 | PASS | PASS | Clean pass |
| 23 | `notes-vault-mcp` 0.7.0 | 156 | `append_file` (one note) → `read_file` | FAIL: 7 of 13, 1 of 10, PASS; 2, 5 and 7 refused | FAIL: 5 of 13, 2 refused | PASS | **Lost updates, false refusals** |
| 24 | `server-noonien` 1.1.0 | 114 | `create_entities` → `read_graph` | PASS ×3 | Incomplete with one shared node id; PASS with one per process | PASS | Clean pass when configured as documented |
| 25 | `@mokei/mcp-sqlite` 0.14.0 (`--db`) | 82 | `sqlite_run` (`INSERT`) → `sqlite_all` | PASS ×3 | PASS, 12 refused (`database is locked`) | PASS | Clean pass |
| 26 | `@pepk/mcp-memory-sqlite` 1.1.1 | 80 | `create_entities` → `read_graph` | PASS ×3 | PASS, 5 refused (`database is locked`) | 1 error, not reproduced in 2 reruns | Pass |
| 27 | `@putervision/state-memory-mcp` 1.4.1 | 78 | `manage_nodes` (`create`) → `manage_nodes` (`list`) | PASS ×3 | PASS | 3 errors | Pass; errors on concurrent first start |
| 28 | `@mnemodb/mcp` 0.1.22 | 73 | `memory_remember` → `memory_list` | PASS ×3 | PASS | PASS | Clean pass |
| 29 | `@psraghuveer/memento` 0.9.2 (`serve`) | 66 | `write_memory` → `list_memories` | PASS ×3 | PASS, 1 refused (`database is locked`) | PASS | Clean pass |
| 30 | `brain-os` 0.10.0 (`serve`, after `brain-os init`) | 60 | `decision_log` → `decision_review` (`limit: 100`) | FAIL: 12, 12, 12 of 15 | FAIL: 5 of 15 | PASS | **Lost updates** |
| 31 | `memory-mcp-lite` 2.0.0 | 33 | `remember_fact` → `search_memory_light` | PASS ×3 | PASS, 11 refused (`SQLITE_BUSY`) | 4 errors | Pass; crashes on concurrent first start |
| 32 | `@jalipata/mcp-memory` 0.1.14 | 20 | `create_entities` → `read_graph` (`limit: 100`) | PASS ×3 | PASS | PASS | Clean pass |
| 33 | `kanbango` 5.3.0 (`kanban mcp`) | 15 | `kanban_manage` (`create`) → `kanban_read` (`list`) | PASS ×3 | PASS | PASS | Clean pass |
| 34 | `@learningnodes/elen-mcp` 0.3.1 | 12 | `elen_commit` → `elen_suggest` (`limit: 50`) | PASS ×3 | PASS | PASS | Clean pass |
| 35 | `@simonblanco/kanban-mcp` 1.6.0 | 9 | `kanban_create_task` → `kanban_list_tasks` | PASS ×3 | Not run: second process exits | Not run: same | Pass in one process |
| 36 | `mcp-simple-memory` 0.4.0 | 8 | `mem_save` → `mem_list` (`limit: 100`) | PASS ×3 | Incomplete; a script kept 3 of 15 | PASS | **Lost updates across processes** |
| 37 | `@chinjinyu/mcp-knowledge-graph-plus` 0.1.7 | 7 | `kg_node_add` → `kg_list` | PASS ×3, 12 refused each run | PASS | PASS | Pass; most parallel writes fail |

`deadlock-probe` found 0 deadlocks and 0 hangs on all 37 servers. Every
probe error above is a tool call that returned an error, listed in the notes.

### Notes per server

- **desktop-commander loses parallel edits to one file.**
  `performSearchReplace` reads the file (`dist/tools/edit.js:110`), builds
  the new content and writes it back (`:160`) with awaits in between and no
  lock. In each pipelined run, every `edit_block` returned success and the
  file held the control and 3 markers, one per round of 5. Across 5
  processes no edit was lost: each process sends its edits one at a time,
  and the window between processes is narrower. This is the same pattern as
  server-filesystem 2.0.0 (2026-10-19), in the most downloaded server of
  this survey.
- **@itseasy21/mcp-knowledge-graph loses entities and can break its file.**
  It is a fork of the reference memory server from before the 1.0.0 lock
  fix: `createEntities` calls `loadGraph` (`fs.readFile`,
  `dist/index.js:24`) and `saveGraph` (`fs.writeFile`, `:44`) with no lock.
  Pipelined, the file held 4 entities after each run (control plus 3). With
  5 processes, `fs.writeFile` truncating the file let two writers interleave:
  the file was left with one valid line and one torn line ending in
  `...}on":1}`, and every later call, including the final read, failed with
  `Unexpected non-whitespace character after JSON at position 145`. The
  store stays unreadable until it is repaired by hand.
- **brain-os loses decisions.** `decision_log` reads every decision
  (`dist/tools/decision-log.js:45-46`), appends one, and `putDecisions`
  rewrites `decisions/decisions.json` (`:80`). The write goes through a
  temporary file and a rename, so the file is never torn, but there is no
  lock. `decisions.json` held 4, 4 and 4 decisions after the pipelined runs
  and 11 after the 5-process run, matching the tool's counts. The package
  documents this itself in `dist/utils/file-store.js:123`: "KNOWN LIMIT: this
  does NOT prevent the load-modify-save race ... tracked as v0.5.1 optimistic
  locking".
- **notes-vault-mcp loses appends and refuses others for a wrong reason.**
  `notes.append` (`notes.py:115`) reads the note, appends, and calls
  `backend.put` without the `expected_version` that `write_file` passes. The
  local backend writes in place with `Path.write_bytes`
  (`backends/local.py:98`), so a concurrent append can read a truncated
  note. That append is refused with `log.md would not be a valid note:
  missing required field: title`, although the note on disk is valid. FastMCP
  runs the synchronous tool in worker threads, so one client's parallel
  calls race. Results varied: 7 of 13 and 1 of 10 acknowledged appends lost,
  then a run where none was lost and 7 were refused.
- **compound-memory keeps every note but its search loses some.** Each
  `memory_write` creates its own Markdown file under a cross-process
  `flock`, and all 16 files were on disk after the 5-process run. The search
  index, `index/tokens.json`, listed only 8 markers, so `memory_search`
  could not find the other 8; a search from a new process did not find them
  either. The write path
  calls `sync()` (`index.py:269`), which reuses the in-memory index loaded
  earlier (`_ensure_live`) and does not reload it when another process has
  written the file since; the read path does (`_ensure_fresh`, `:180`, whose
  docstring calls cross-process concurrent writes out of scope). Pipelined
  runs pass. The loss repeated in a second 5-process run (8 of 15).
  `compound-memory rebuild-index` should restore the entries; that was not
  run.
- **mcp-simple-memory overwrites the database from each process.** It loads
  `memory.db` into sql.js once at start-up (`dist/index.js:36-39`) and every
  write saves the whole in-memory database back (`persist()`, `:91-92`).
  Pipelined runs pass. With 5 processes, the control read on a second
  process could not see the first process's write, so `lost_update`
  reported the run as incomplete instead of as lost updates; see the tool
  defects below. A separate script started 5 processes, initialized all of
  them, sent 3 `mem_save` calls on each, and read from a fresh process: all
  15 calls succeeded and 3 were kept, all from one process, in 3 of 3 runs.
- **@chinjinyu/mcp-knowledge-graph-plus fails parallel writes instead of
  losing them.** All repositories share one `sqlite3` connection and each
  call issues `BEGIN TRANSACTION`
  (`dist/infrastructure/repositories/BaseRepository.js:42`). Inside one
  process, 12 of every 15 parallel `kg_node_add` calls returned `SQLITE_ERROR:
  cannot start a transaction within a transaction`. Every acknowledged write
  was read back, so the check passes, but a client that runs tool calls in
  parallel sees most of them fail.
- **Errors on concurrent first start.** The probe starts 5 processes on an
  empty store at once. Three servers fail some of those calls because each
  process creates or migrates the schema:
  `@ai-devkit/memory` (a separate script: 4 of 5 calls returned `duplicate
  column name: embedding`), `@putervision/state-memory-mcp` (3 of 5: `table
  schema_meta already exists`), and `memory-mcp-lite` (2 of 5 processes
  exited with `fatal error: LibsqlError: SQLITE_BUSY: database is locked`).
  `@pepk/mcp-memory-sqlite` had 1 error of 5 once; two more probes passed,
  and the error text was not recorded (see the tool defects). None is a
  deadlock, and none loses data.
- **SQLite servers refuse rather than lose.** Six servers answered some of
  the 5-process writes with `database is locked` or `SQLITE_BUSY` (actari,
  `@ai-devkit/memory`, `@pepk/mcp-memory-sqlite`, `@mokei/mcp-sqlite`,
  memory-mcp-lite, `@psraghuveer/memento`) and kept every acknowledged one.
  Without a busy timeout or retry, a client sees those as failures.
- **Single-process by design.** mcp-server-motherduck uses DuckDB, which
  locks the database file for one process: every write from the other 4
  processes was refused with `Could not set lock on file`, and 4 of 5 probe
  calls failed the same way. `@simonblanco/kanban-mcp` starts a web viewer
  on fixed port 3456; a second process exits with `EADDRINUSE`, so neither
  the 5-process run nor the probe could start. server-noonien's own error
  message asks for a unique `NOONIEN_NODE_ID` per process; with the default
  shared id, 4 writes were refused (`two writers used the shard "vm.jsonl"`) and the final
  read failed. A wrapper giving each process its own id passed.
- **mama-server, 5 processes.** The first `save` in each process loads an
  embedding model (553 MB, cached in advance and linked into each scratch
  `HOME`). With 5 processes loading it at once, every first write exceeded
  the hang threshold, so the run is inconclusive. One process passed 3 of 3
  runs, and the probe on `search` passed.
- **Wrong read arguments caught during verification.** Two first attempts
  failed with the same count every run and the control write lost each
  time: `@jalipata/mcp-memory` `read_graph` returns 10 entities by default
  and `@vibeflow-tools/cli` `list_tasks` returns 5. With `limit: 100` and
  `limit: 0` both pass; only those runs are in the table. graph-mind-memory's
  first probe used `brain_recall` with `recent: true`, which fails with
  `unable to open database file` on an empty store even in one process, so
  it is not a concurrency error; the probe in the table used the read
  arguments of the check.

### Skipped

| Server | Reason |
|---|---|
| server-memory, server-filesystem, mcp-knowledge-graph, mcp-taskmanager, mcp-shrimp-task-manager, basic-memory, mnemonic-mcp, obsidian-mcp, mcp-memory-keeper, `@iachilles/memento`, mcp-server-sqlite | Already in this file |
| `obsidian-mcp-server` 3.7.0 | Needs `OBSIDIAN_API_KEY` and the Obsidian REST plugin |
| `cortex-memory-mcp` 1.2.6 | Needs `CORTEX_API_KEY` |
| `usetrail` 1.29.1 | Every tool needs a login to its hosted API |
| `lodekeep` 0.3.6 | Needs a lodekeep.com account |
| `mcp-notes` 1.0.5 | Stores notes in DynamoDB |
| `@qianshitech/hypermarrow-mcp` 0.1.1, `fleet-memory-mcp` 0.1.0 | Thin clients for a separate local daemon (ports 8741, 5100) |
| `todoglow-mcp` 1.1.2, `noglow-mcp` 0.2.1 | Talk to a desktop app; noglow has no read tool |
| `@wrongstack/kanban-mcp` 1.1.8 | Read-only tools over the WrongStack app's IPC |
| `@br8n/mcp` 0.1.2, `@database-mcp/sqlite` 0.5.1 | Read-only: no write tool |
| `@blizzy/mcp-task-manager` 0.12.1 | No tool lists all tasks (`task_info` needs the ids) |
| `mcp-memory-bucket` 0.15.1 | Needs folders configured through its web UI on fixed port 8767 |
| `kanban` 0.1.70 | Its `mcp` command prints "Deprecated" |
| `memcell` 1.6.1 | Install fails: the published package depends on `workspace:*` |
| `mcp-server-memory` 1.0.3 | `tools/list` returned no tools |
| `@berthojoris/mcp-sqlite-server` 1.4.1 | No reply to `initialize` within 90 s |
| `fronyboard` 0.39.2 | `open_period` and `create_task` failed with no error text for every period format tried |
| `exomem` 0.132.2 | Needs a vault with a schema contract; setup not attempted |
| `openmymemo` 0.2.9 | No stdio MCP command found in its CLI help |
| `omega-memory` 1.3.0 | The registry version is not on PyPI |
| `pmb-ai` 1.1.0, `cuba-memorys` 1.32.0 | Pull PyTorch/CUDA or multi-GB models; the first filled the container's disk |
| `@gannonh/memento-mcp` 0.3.9, `@sylweriusz/mcp-neo4j-memory-server` 3.2.0 | Need Neo4j |
| `memory-journal-mcp` 8.0.1, `@j0hanz/memdb` 1.4.0, `@izumisy/mcp-duckdb-memory-server` 1.1.10, `mcp-sqlite-tools` 0.1.1, `aimem-mcp` 0.3.0, `@raajvamsy/memorylayer` 1.7.1 | Declare a Node version other than 22 (>= 24, 20 only, or < 23); not attempted |
| `codebase-memory-mcp`, `gemdex-mcp`, `@tpsdev-ai/flair-mcp`, `llmslim` | Code index, macOS-only (MLX), needs the Flair server, or a library rather than a stateful server |

### Tool defects found

| Defect | Status |
|---|---|
| `deadlock-probe` drops the text of a tool error. A call answered with `isError` counts as `ServerError`, and neither the summary, `report.md` nor `metrics.json` says what the server returned (`@ai-devkit/memory`, `@pepk/mcp-memory-sqlite`, memory-mcp-lite). JSON-RPC errors are quoted in the notes (state-memory), and `lost_update` quotes `isError` text. The pepk error could not be identified because of this. | Fixed: each `isError` reply adds a note with the server's text, escaped and capped at 200 characters |
| `lost_update` misses cross-process loss when each process keeps its own copy of the store. With mcp-simple-memory, the control read on a second process cannot see the first process's write, so the run is `incomplete` while the server keeps 3 of 15 acknowledged writes. A final read from a new session, after the writers have exited, would see the loss. | Fixed: the control and final reads now come from sessions started after the writes ([ADR 0036](adr/0036-lost-update-reads-from-fresh-sessions.md)) |
| `--server` is documented as "parsed shell-style" in every subcommand's help, but `split_server_command` (`crates/core/src/config/load.rs:22-37`) splits on whitespace and ignores quotes. `--server "sh -c 'exec npx ...'"` hands `sh` the argument `'exec`, which fails with `Unterminated quoted string`. The comment on the function says so; the help text does not. | Open |
| `lost-update` has no `--hang-threshold`, and `deadlock-probe` has no `--env` or `--startup-timeout`. mama-server's 5-process run was inconclusive because of the fixed threshold; the survey wrapped every command in `env` to set the store. | Open, minor |
| Any `isError` reply counts as a refused write (ADR 0032), so `@chinjinyu/mcp-knowledge-graph-plus` passes while 12 of 15 parallel writes fail with an internal SQLite error, and notes-vault-mcp passed one run with 7 false refusals. The notes show the refusal text, so this is visible but not gated. | Design question, not a defect under ADR 0032 |

### Gate status

37 servers tested. Six lose acknowledged writes: desktop-commander,
`@itseasy21/mcp-knowledge-graph`, brain-os and notes-vault-mcp inside one
process, compound-memory (search index) and mcp-simple-memory only across
processes. Five of the six were found by `lost-update` itself; the
mcp-simple-memory loss needed a script. No deadlock or hang was found. None
of the six has been reported upstream. Four tool defects were open; the
cross-process one is fixed since (ADR 0036), and a rerun on 2026-10-21
counted 12 of 15 writes lost in each of 3 runs against mcp-simple-memory. The
summary for outside readers is [survey-2026-10.md](survey-2026-10.md).
