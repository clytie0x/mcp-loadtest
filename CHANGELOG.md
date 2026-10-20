# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `crates/engine/tests/taskmanager_regression.rs`: an ignored regression test
  that runs `lost_update` against `@kazuph/mcp-taskmanager` 1.1.1, pinned on
  npm, in pipelined and multi-process modes and expects lost updates.
- `crates/engine/tests/filesystem_regression.rs`: the same ignored
  regression test against `@modelcontextprotocol/server-filesystem` 2.0.0,
  where parallel `edit_file` calls on one file lose edits.
- The README lists the servers `lost-update` has found losing writes.
- `docs/examples/ci-gate.md`: adding `deadlock-probe` and `lost_update` to a
  server's pull-request workflow through the Action.

## [0.6.0] — Release contents

This section describes the source contents intended for `v0.6.0`. Its presence
does not prove that the tag, checksums, platform archives, or GitHub Release
have been published; verify those artifacts independently.

### Fixed

- The stdio transport now answers requests the server sends to the client
  while a call waits: `ping` gets an empty result, and any other method
  (`roots/list`, sampling, elicitation) gets JSON-RPC method-not-found.
  Before, such a request was read as the call's response and failed the
  session with "JSON-RPC response requires exactly one of `result` or
  `error`", so a server like mcp-shrimp-task-manager, which holds
  `tools/list` until its `roots/list` is answered, could not be tested
  (#40).

### Added

- `lost-update --pipelined` (`pipelined` in config and `serve --mcp`) checks
  for writes lost inside one server process. One session sends each round of
  `--concurrent` writes back to back on a stdio stream and matches the
  replies by id, the way clients that run tool calls in parallel do
  (ADR 0033). It found the server-filesystem and mcp-taskmanager losses that
  the default mode could only show across processes.
- A pinned fixture for the truncate-then-rewrite store that
  server-filesystem 1.0.0 introduced (`mock-store.py --mode inplace`), which
  `lost_update` must catch dropping data stored before the run.

## [0.5.0] — Release contents

This section describes the source contents intended for `v0.5.0`. Its presence
does not prove that the tag, checksums, platform archives, or GitHub Release
have been published; verify those artifacts independently.

### Changed

- `lost_update` no longer fails a server for refusing a write. A concurrent
  write whose result has `isError` counts toward a new `refused_write_count`
  (metrics.json, `serve --mcp`) instead of making the check incomplete; every
  acknowledged write must still be read back. The notes now quote the
  server's error text for a refused write, control write, or read
  (ADR 0032, #31).

### Fixed

- On macOS, a launched server's child that was killed while its launcher was
  still alive could be reported as surviving the kill, failing teardown.
  sysinfo reads a process's status only when it first sees it, so the check
  now re-reads it on every poll.

## [0.4.1] — Release contents

This section describes the source contents intended for `v0.4.1`. Its presence
does not prove that the tag, checksums, platform archives, or GitHub Release
have been published; verify those artifacts independently.

### Fixed

- The composite Action downloaded its binary from
  `Teerapat-Vatpitak/mcp-loadtest`, the repository's name before its owner
  was renamed to `clytie0x`. GitHub redirects the old name for now, but
  whoever registers it later would control which binary the Action installs,
  and the `.sha256` comes from the same place, so the check would not catch
  it. The Action, README, docs, crate metadata, and schema `$id`s now use
  `clytie0x/mcp-loadtest`.

### Changed

- The README Install section gives the download, checksum, and PATH steps
  for each platform instead of only the Cargo command.

## [0.4.0] — Release contents

This section describes the source contents intended for `v0.4.0`. Its presence
does not prove that the tag, checksums, platform archives, or GitHub Release
have been published; verify those artifacts independently.

`deadlock-probe` no longer defaults `--tool` to `echo`; see **Changed**.

### Changed

- `deadlock-probe` no longer defaults `--tool` to `echo`, which most real
  servers do not have. Without `--tool` it lists the server's tools on one
  session (following `nextCursor`), shuts that session down, and probes the
  first tool the server marks read-only (`readOnlyHint: true`, not
  `destructiveHint: true`) that has no required arguments, with `{}`. It
  prints the choice and that the marks are the server's own hints. When no
  tool qualifies it stops with exit code 1 and a list of the tools.
  `--args` now requires `--tool`. If you relied on the implicit `echo`, pass
  `--tool echo`. See [ADR 0030](docs/adr/0030-deadlock-probe-picks-a-tool.md).
  The library gains `run::list_all_tools`, `Session::list_tools_page` and
  `Session::list_all_tools`.
- `lost_update` also checks the control write in the final read. If it is
  missing, the store dropped data it already held (for example, a writer
  that read a truncated file), and it counts as a lost update with its own
  note. mcp-knowledge-graph 1.3.2 lost its control write on every run.

### Added

- `list_target_tools` returns each target tool's `annotations` (null when
  absent), so an agent can narrow its choice to read-only tools for
  `race_check` and `sustained_load`. The description calls them the target's
  hints, not guarantees, and still requires disposable or modifiable data.
  `protocol::mcp::Tool` gains an `annotations` field.
- `race-check` and `lost-update` subcommands run those detectors without a
  config file, like `deadlock-probe`. Both take `--env KEY=VALUE` and
  `--startup-timeout`, write the usual reports, and exit non-zero with the
  failure reasons.
- `serve --mcp` returns `instructions` at initialize: which tool answers
  which question, the order to call them in, and that run tools make real
  calls. MCP clients pass these to the model. The README shows how to
  register the server with an agent.
- `serve --mcp` tools can target a remote server: pass `server_url` (with
  optional `transport`, `allowed_hosts`, and `headers_from_env`) instead of
  `server_command`. Remote targets go through the same SSRF guard, header,
  and OAuth handling as a config file. `lost_update` over HTTP is the
  natural case, with one server process and many sessions sharing its
  memory. The library gains `run::open_session` to open one session the way
  a run does.
- `serve --mcp` tools that spawn the target accept `env`, `working_dir`, and
  `startup_timeout_ms`. An agent can now probe servers that read their
  configuration from the environment (a data file path, an API key) and
  servers that are slow to start, such as `npx` downloading on first use.
- The release workflow names the cause when its immutable-release audit
  token is rejected: expired or revoked (HTTP 401), or missing access or
  permission (HTTP 403/404). It used to fail with gh's bare
  `Bad credentials`.
- `tag-release` workflow: dispatched from `main` with a version and a
  commit, it checks that CI passed on the commit, creates the annotated
  release tag through `scripts/release-tag.sh`, and dispatches `release.yml`.
  Maintainers, or an agent acting on their confirmation, can release without
  pushing a tag from a local clone. See
  [ADR 0028](docs/adr/0028-dispatched-release-tagging.md).

### Fixed

- A run's `tools/list` registry and `serve --mcp`'s `list_target_tools`
  follow `nextCursor`. They read only the first page before, so a server
  that pages its tools lost the later ones from coverage, from strict-mode
  schema validation, and from the agent's tool list.
- Shutting down a server started through a launcher (`uvx`, `npx`, a shell
  script) also kills the server the launcher started. The forced-kill and
  drop paths used to kill only the launcher, so a server that ignored stdin
  EOF kept running after the run ended. A lost-update run against
  mcp-server-sqlite through `uvx` left 12 server processes behind.
- The stderr capture pump writes to the log file in 64 KiB blocks instead of
  once per line. A server that logged a burst used to block on its full
  stderr pipe while the pump caught up: its latency was skewed, and at
  shutdown it could miss the 5-second graceful-exit window and be killed with
  part of its log unwritten. The file is still flushed whenever the server
  goes quiet, so it can be tailed live, and a transport dropped without
  `shutdown` gives the pump up to two seconds to write what it has read.
- `failure_reasons` names every way a run can fail. A `race_check`,
  `deadlock_probe`, or `lost_update` run with a response past the hang
  threshold failed with an empty list, so `serve --mcp` returned
  `passed: false` with no reason and the CLI printed `failed — ;`. A
  race-check worker that did not finish without recording an error, and a
  report claiming more successes than attempts, were also unnamed.

- `protocol_version = "auto"` reaches stdio servers built on older Python MCP
  SDKs. They exit on the unknown `server/discover` probe instead of answering
  -32601, so every session failed with "connection closed" until the version
  was pinned. A connection that closes before the session is established now
  restarts the server once with the legacy handshake, and a one-time warning
  names the pin that avoids the extra start. Explicit versions never fall
  back, and a traced run fails with the same advice instead of recording a
  trace that could not be replayed. See ADR 0029.

## [0.3.0] — Release contents

This section describes the source contents intended for `v0.3.0`. Its presence
does not prove that the tag, checksums, platform archives, or GitHub Release
have been published; verify those artifacts independently.

This release narrows the tool to MCP concurrency-bug detection (ADR 0024,
ADR 0025) and is breaking: see **Removed**.

### Added

- `lost_update` scenario and `serve --mcp` tool: concurrent sessions send
  writes that each carry a unique `{marker}`, then one read must return every
  acknowledged marker. Missing markers set `lost_update_count`, which fails
  the run and appears in `metrics.json`, Prometheus, JUnit, and
  `failure_reasons`. A control marker written on one session must be visible
  to a read on another before any concurrent write; otherwise the check is
  inconclusive (the read cannot observe writes, or sessions do not share
  state). See
  [ADR 0027](docs/adr/0027-lost-update-detector.md).
- `serve --mcp` run tools take an optional `report_dir` and return the
  `run_dir` holding the run's artifacts.
- The official conformance runners pin `modelcontextprotocol/conformance`
  `c37eec8` (was `49103de`) and also execute
  `json-schema-2020-12-preservation`; all 33 client scenarios pass. See
  [ADR 0026](docs/adr/0026-advance-conformance-pin-c37eec8.md).
- `serve --mcp` tools for agents: `list_target_tools` returns the target's
  `tools/list` so an agent can pick a tool to probe, `race_check` runs the
  divergence detector, and `cold_start` times the handshake and the first
  call separately. Every run tool now returns `failure_reasons`, a list of
  the signals that failed the run (empty when `passed` is true), plus the
  shared counters and `notes`. Existing output keys are unchanged.
- `Report::per_tool` carries per-tool metrics, which the executor already
  computed for coverage and per-tool SLOs.
- `mcp_loadtest::report::failure_reasons` exposes the reasons the terminal
  and Markdown reports print after `FAIL`.

### Fixed

- OAuth `resource` parameters now carry the protected-resource metadata
  identifier byte for byte. The client re-serialized it through `url::Url`,
  which appends `/` to a pathless origin such as `https://mcp.example`, so
  authorization, token, refresh, and client-credentials requests named a
  different resource than the server published. Exposed by the new
  `resource-parameter-matches-prm` check in the official conformance suite.
  `AuthorizationContext::resource_identifier` returns the exact value.
- The stdio transport no longer corrupts multi-byte UTF-8 characters that
  straddle two 8 KiB reads. It decoded each chunk separately, so a Thai or
  CJK character on a chunk boundary in a long response became U+FFFD, which
  could also make `race_check` report a false divergence. The reader now
  collects raw bytes and decodes each line once, which also drops a
  per-chunk allocation.
- `[server.env]` and `[server] working_dir` were parsed but never applied, so
  every stdio server (the run session and every pooled session) started with
  the parent's environment and directory. `SpawnOptions` now carries both and
  `run` passes them on each spawn. Found during Phase 1 field runs against
  `@modelcontextprotocol/server-memory`, whose `MEMORY_FILE_PATH` was ignored.
- Auto protocol negotiation no longer kills a session whose server answers
  the `server/discover` probe after the 5 s probe window. The late reply used
  to be read as the `initialize` response and failed as an id mismatch
  (reported as `Malformed`). It is now read before the `initialize` result:
  a late error is skipped and the legacy handshake continues, while a late
  successful discovery is adopted as a 2026-07-28 session when the server
  then refuses `initialize` (-32022). Found during Phase 1 field runs:
  10 or more concurrent `mcp-server-git` processes hit the first case, and
  `markitdown-mcp` (about 5 s to start) hit the second on every run.
  New `Transport::recv_response` (default: unsupported) lets stream
  transports read that reply.
- Reports no longer print zeroed process stats as readings. When no process
  samples were taken (pooled stdio runs, whose workload runs outside the
  sampled PID), the Markdown report says "Process metrics were not
  sampled", the terminal summary says `process: not sampled`, and
  `metrics.json` adds `"sampled": false` to `process`. Scenario notes,
  including the reason metrics are unavailable, now appear in a Markdown
  `## Notes` section, as `note:` lines in the terminal summary, and as an
  optional `notes` array in `metrics.json`. Both JSON fields are additive
  and omitted in the common case. Found during Phase 1 field runs, where
  pooled stdio runs showed `Peak RSS: 0.0 MB · Avg CPU: 0.0%`.
- The Markdown Errors table and the terminal `errors:` line now list hangs
  and deadlocks. The error rate already counted them, so a run with only
  hangs showed a non-zero error rate next to "No errors recorded". Found
  during Phase 1 field runs (3 hangs of 5 calls, `Error rate: 60.00%`).
- The `serve_smoke` test now honours `CARGO_TARGET_DIR` when locating and
  building the CLI binary (#5).

### Removed

- Distributed load generation over OpenSSH workers: the
  `mcp-loadtest-distributed` crate, the hidden `__distributed-agent`
  subcommand, the `[distributed]` config block, and the engine's
  coordinated traffic-start gate and seeded pattern selection that only
  distributed workers used.
- The OTLP/HTTP metrics exporter and the `[output.otlp]` config block.
  JSON, JUnit, and Prometheus reports remain.
- Both blocks are also gone from the bundled config schema. Configs that
  still contain them are not rejected; the blocks are ignored.
- Features outside the concurrency-bug scope, per
  [ADR 0025](docs/adr/0025-trim-to-detector-surface.md): the `ramp` (with
  breaking-point analysis), `spike`, `soak`, and `fuzzer` scenarios; the
  `ws` WebSocket transport; the `cross` subcommand and A-F grading; the
  `html` report format; the unused Ratatui dashboard; and the
  `[output.history]` rolling baseline. Configs that use any of them now
  fail validation, and the GitHub Action rejects `command: cross`. Use
  `compare` against a stored `metrics.json` for regression gating.
- `metrics.json` no longer emits `expected_rejection_count`, and the
  Prometheus output drops the `expected_rejection` outcome label. Only the
  fuzzer produced them.
- `Transport::raw_send` and `Session::raw_send`, which only the fuzzer used.
- Unused dependencies: `ratatui`, `crossterm`, `tokio-tungstenite`,
  `tokio-stream`, `indicatif`, `futures`, and `ulid` in the CLI.

## [0.2.0] — Release candidate contents

This section describes the source contents intended for `v0.2.0`. Its presence
does not prove that the tag, checksums, platform archives, or GitHub Release
have been published; verify those artifacts independently.

### Added

- Distributed load testing over ephemeral OpenSSH workers with a versioned
  `mcp-loadtest-dist/1` control protocol, deterministic global-concurrency
  sharding, coordinated readiness/start gates, heartbeat failure detection,
  and exact cross-agent HDR histogram aggregation.
- A bundled Draft 2020-12 configuration schema, available through
  `mcp-loadtest config-schema`, for editor completion and validation.
- JUnit XML and Prometheus text report files plus a bounded, retrying
  OTLP/HTTP metrics exporter. Remote exporters use environment-backed headers,
  redirect denial, DNS/address pinning, response limits, and explicit
  fail-on-error policy.
- Rolling baseline history with atomic sample publication, rolling-median
  trends, warm-up policy, topology fingerprints, and CI gates for p99,
  error-rate, throughput, and deadlock regressions.
- Full client-role support for the final MCP `2026-07-28` revision, including
  stateless discovery, required HTTP metadata, multi-round tool requests, and
  local Draft 2020-12 schema validation without network `$ref` retrieval.
- OAuth protected-resource/authorization-server discovery, pre-registered
  clients, Client ID Metadata Documents, optional dynamic registration,
  authorization-code + PKCE loopback callbacks, client credentials, refresh,
  and bounded insufficient-scope step-up. Secrets remain environment-backed
  and distributed workers acquire their own tokens locally.

### Security

- OAuth metadata and token endpoints are resolved, checked against
  private/reserved address ranges, and pinned per request; redirects and proxy
  re-resolution are disabled.
- Distributed jobs never carry literal credentials or tokens. Agent-local
  target policy can only narrow the controller request, and incomplete cohorts
  fail closed without aggregate evidence.

### Changed

- Changed the license for development after `v0.1.0` to Apache-2.0. The
  immutable `v0.1.0` release retains its original `MIT OR Apache-2.0` terms.

## [0.1.0] — Release contents

This section describes the `v0.1.0` source contents. Its presence in a checkout
does not prove that a tag, GitHub Release, prebuilt binary, or Action release
exists; verify the exact tag, published assets, and GitHub immutable-release
status externally. The release workflow refuses publication while immutable
releases are disabled.
crates.io is not a `v0.1.0` distribution channel. The earlier `0.0.1`
workspace version was never tagged or released.

### Security

- Removed shell evaluation from the composite GitHub Action. The `args` input
  is now a JSON array of strings, decoded into literal argv entries; invalid
  JSON, non-string entries, NUL bytes, and shell syntax fail closed.
- The Action now defaults the installed binary to `v0.1.0` and accepts only a
  resolved stable `vMAJOR.MINOR.PATCH` tag before writing to GitHub environment
  files, blocking multiline command-file injection and mutable-ref surprises.
- Added `server.headers_from_env` for HTTP/SSE/WebSocket authentication
  headers. Configuration stores only environment-variable names; resolved
  secret values are not included in config debug output or error messages.
  Static outbound headers are the complete remote-auth scope for
  `v0.1.0`—OAuth login, refresh, discovery, and interactive authorization
  are not implemented. All `MCP-*`, `Sec-WebSocket-*`, hop-by-hop, and proxy
  headers remain transport-owned and cannot be overridden. Supplying any
  remote header requires `https://` for HTTP/SSE or `wss://` for WebSocket;
  URL userinfo is forbidden. Query strings still reach the target but are
  redacted wholesale in reports/traces and must not carry credentials.
- Remote URL validation is now shared by config parsing and the direct
  HTTP/SSE/WebSocket constructors, including the server-selected SSE POST
  endpoint. Unknown `[server]` keys fail closed without echoing literal
  values, case-insensitive duplicate header names are rejected at connect,
  and secret-backed header values remain marked sensitive in both reqwest and
  WebSocket requests.

### Fixed

- Stdio `Session` now tolerates server-initiated JSON-RPC notifications
  (`notifications/tools/list_changed`, progress updates, etc.) interleaved
  with responses — including a notification emitted before the `initialize`
  result. Previously the transport read the next line as the response, so a
  leading notification desynced the stream and every call was misclassified
  `malformed` (found by dogfooding against the MCP reference "everything"
  server: 100% malformed → 0 errors after the fix). The response read now
  skips notification frames (a `method`, no `id`) up to a bounded cap.
- JSON-RPC responses now fail closed unless `jsonrpc` is exactly `"2.0"`,
  the response id is a valid JSON-RPC string/number/null value, and the
  envelope contains exactly one of `result` or `error`. Mismatched-id success
  payloads and structured errors are retained internally so the raw fuzzer can
  distinguish an explicit allowed rejection from a server that accepted
  malformed input.
- `compare` now exits non-zero when any regression flag fires, as `--explain`
  and the composite GitHub Action's gating contract (DESIGN.md §15.4) always
  documented. Previously it printed the diff and exited 0, so regressions
  passed CI. The diff is still rendered to stdout first; the failure line
  names the regressed metrics.
- Config validation now rejects `transport = "ws"` without `server.url` at
  load time (previously the missing URL only surfaced at connect).
- `deadlock_probe` now completes all N independent session handshakes before
  a shared start gate releases one call per worker when `concurrent > 1`.
  `concurrent = 1` keeps the focused borrowed-session path; a direct library
  context without `SessionFactory` rejects N>1 instead of silently
  serializing. `--explain` documents the synchronized behavior and the
  quick-subcommand vs `run`-config defaults
  (5 / 2s / 5s vs 20 / 5s / 10s).
- Pattern workloads now use the same real N-session pool as the single-tool
  sustained path when `concurrent > 1` and a `SessionFactory` is available.
  A direct-library context without a factory reports its sequential fallback
  instead of silently claiming N-way concurrency.
- `race_check` now requires at least two independent sessions, completes all
  handshakes before releasing one identical call per worker through a shared
  gate, and records response divergence as a first-class CI failure. A missing
  or errored worker makes the cohort inconclusive and fails closed; partial
  responses are never reported as a clean no-divergence result.
- `Report::passed()` now rejects no-op runs, all-failed runs, deadlocks,
  response divergences, incomplete pooled-worker/race-check cohorts, and
  uncertain session teardown. It also rejects recorded deadlock, timeout,
  protocol/malformed, crash, disconnect, or cancellation outcomes even when
  no threshold was configured. Any error or completed call that breaches the
  hang threshold in a `race_check`, `deadlock_probe`, or `fuzzer` diagnostic
  also fails—even in a mixed cohort with surviving successes. Partial slow
  calls and application errors in normal load scenarios remain governed by
  configured threshold policy.
- Session teardown now closes stdin, applies bounded graceful-exit and
  forced-kill/reap phases, joins or aborts the stderr pump without detaching
  it, and records any teardown error/timeout as a typed report-gating signal.
  Run duration is finalized after shutdown; trace replay also exits non-zero
  when target teardown is uncertain. Scenario and test outer deadlines leave
  scheduling margin above the transport's composed internal budget.
- Captured stderr for pooled and cold-start factory sessions now uses one
  immutable file per session under
  `runs/<id>/server-stderr/session-NNNNNN.log`; concurrent workers can no
  longer truncate the shared initial `server-stderr.log`.
- Every run now fails before scenario traffic when its initial `tools/list`
  discovery cannot be obtained. A server that rejects discovery but accepts a
  known `tools/call` can no longer report PASS; strict mode additionally
  reuses the successful registry for schema validation.
- Configured process-memory thresholds fail closed when process samples are
  missing, insufficient, non-finite, or unusable; config parsing also rejects
  NaN and positive/negative infinity. Unset process thresholds remain
  best-effort and do not gate platforms without observable process data.
- Configured global latency/error gates and per-tool SLOs now require recorder
  samples. A custom or incomplete scenario can no longer satisfy a threshold
  with default-zero metrics, and an unexercised tool SLO is a typed violation.
- Process metrics are cleared and configured process thresholds fail closed
  when pooled, cold-start, version-matrix, or fuzzer traffic creates stdio
  factory children outside the current single-PID sampler; an idle initial
  child can no longer produce PASS.
- `cold_start` rejects `warmup = true` with fewer than two iterations because
  that plan has no measured evidence. Zero-length optional spike phases are
  skipped instead of spawning workers that can only look incomplete.
- Healthy protocol-fuzzer rejection now has its own `ExpectedRejection`
  metric. It counts as a successful fuzz probe without masking genuine
  protocol/malformed errors or client-side schema/version failures.
- The normal stdio fixture now returns JSON-RPC `-32700` for malformed JSON
  instead of crashing. A repeat-suite transient showed the old crash racing
  the raw fuzzer's liveness deadline and appearing nondeterministically as
  either `Disconnected` or `Deadlock`; the regression now requires an explicit
  rejection with zero disconnects and zero deadlocks.
- Process-heavy nextest contention pushed pooled Python fixture initialization
  past the test default, so the deadlock integration test now uses a test-local
  30-second startup budget. The production 10-second default and strict
  complete-worker/pass/fail assertions are unchanged.
- The no-retry release repeat gate exposed nine Windows stdio shutdown
  failures across two of five attempts. Tokio's registered process-wait
  callback could lag the process table during the 24-way subprocess wave, so
  an accepted termination was misreported as a 2-second forced-reap timeout.
  Stdio teardown now reconciles both graceful and forced async-wait failures
  with a direct `try_wait` probe: a proven exit/reap succeeds, a still-live
  process or failed probe remains a typed failure. A follow-up strict run
  proved that some processes really remained live after the old 2-second
  forced budget, so forced reap now has a bounded 5 seconds and its outer
  lifecycle guards retain scheduling margin. Nextest limits engine
  integration-test binaries to four at once while a focused regression still
  drives eight stubborn child shutdowns concurrently; scenario-level
  concurrency tests retain their real worker counts. The release procedure
  keeps the original failed JUnit and logs as unresolved local evidence rather
  than discarding them.
- The hang watchdog now classifies successful calls from elapsed wall time,
  not merely the winning `select!` branch. Under a saturated executor the call
  future and threshold timer could become ready in the same poll; the old
  future-first bias produced `Ok` after the threshold. Deterministic scheduler
  stall regressions require `Slow` or `Deadlock` at the real deadlines.
- A Run's `server.startup_timeout` is one deadline across transport
  connect/spawn, initialize or stateless discover, and the mandatory initial
  `tools/list`; strict factory workers reuse the same rule. Header/upgrade and
  tools-list stalls now return the configured typed timeout and run bounded
  teardown instead of hanging without a report.
- Remote reader failures are latched outside the foreground response queue.
  SSE/WS aggregate-budget, parser, socket, and unexpected-close failures that
  arrive after a matching response therefore still gate teardown instead of
  being hidden by an earlier success.

### Changed

- Workspace version is `0.1.0`; a manifest version alone is not evidence that
  an external release exists.
- The GitHub Action's `args` contract intentionally breaks from shell-quoted
  text to a JSON array of strings. Callers must migrate, for example:
  `args: '["--tool","echo","--args","{\"message\":\"hi\"}"]'`.
- Grading: the concurrency dimension's note now discloses inline that
  `total_requests` is a proxy for concurrency capacity (e.g.
  `sustained requests 1234 -> A (>= 100; proxy for concurrency)`).
- CI/repeat-test scripts emit JUnit plus per-attempt logs under
  `target/nextest/` and `target/test-artifacts/`; CI uploads those diagnostics
  even when a test attempt fails. Repeat output directories cannot retain a
  stale per-run JUnit file, and a failed log write fails the Bash evidence
  run. Each evidence set also records tool/OS/git status and Git object hashes
  for every tracked or untracked source file, so a transient can be tied to
  the exact dirty source tree without copying source contents.
  `DISPOSITION.txt` marks a green no-retry cohort or makes failed attempts an
  explicitly unresolved release blocker pending root-cause analysis.
- Captured stderr keeps the orchestrator's initial session at
  `runs/<id>/server-stderr.log` and writes factory-spawned pooled/cold-start
  sessions to unique files below `runs/<id>/server-stderr/`.
- Release automation now grants write permission only to one final publisher:
  every platform build is collected first, all four archives and checksums are
  verified, and a draft is made public only after all eight assets exist.
  Workflow actions are pinned to reviewed commit SHAs.

### Added

`v0.1.0` is a cross-platform load tester and bug detector
for MCP (Model Context Protocol) servers. Its primary promise is deterministic
CI gating for protocol-level hangs, deadlocks, response divergence, terminal
transport failures, and configured performance thresholds.

#### Protocol & transports

- JSON-RPC 2.0 / MCP protocol stack with a zero-copy hot path (ADR 0006):
  borrowing request types, `Session::call_tool(&str, &Value)` — no per-call
  deep clone of the args tree.
- `Session` — spawn/connect, `initialize` handshake, `tools/list`,
  `tools/call`, and shutdown; plus `SessionFactory`, a public cloneable
  factory producing fresh sessions over the run's configured transport
  (version-aware via `SessionFactory::with_version`). Run applies
  `server.startup_timeout` across transport connect, initialize/discover, and
  its required initial `tools/list` rather than resetting or dropping the
  deadline between phases.
- Four transports behind the `Transport` trait: stdio (child process spawn,
  `SpawnOptions` stderr disposition — ADR 0013), Streamable HTTP, HTTP+SSE,
  and WebSocket (rustls). Remote-controlled HTTP response bodies, SSE event
  data, and WebSocket messages are rejected above 16 MiB while the network
  stream is being consumed; SSE/WS also enforce a 32 MiB aggregate byte
  budget across their reader queues and id-mismatch buffers.
- Multi-version MCP protocol support (ADR 0018): typed `ProtocolVersion` enum
  covering `{2025-03-26, 2025-06-18, 2025-11-25}`; optional
  `[server] protocol_version = "auto" | "<rev>"` pin (useful for CI
  version-matrix runs); default advertised revision `2025-11-25`;
  `MCP-Protocol-Version` header on Streamable HTTP (2025-06-18+ requirement).
  A server answering with a revision outside the supported set logs a warning
  and, under `[validation] strict = true`, fails the run before any scenario
  traffic.
- Experimental stateless `2026-07-28` connection mode (ADR 0019), selected
  only by an explicit version pin and never used as the default. The
  implemented subset is reconciled to the final specification tag at commit
  `5f5440bb26a62e2cf3440b92da5a667efa03b267` and conformance commit
  `49103de6ed70804e940637bf3e9e29e4a3f54e64`. The final definitions used by
  this client match the reviewed pre-final schema definitions, so no
  tools/discovery/request-header wire change was required; the final-only
  `subscriptions/listen` response-envelope delta is outside the implemented
  scope. It sends no `initialize`, follows the final request
  metadata/discovery contract, and is scoped to stdio + Streamable HTTP
  (SSE/WS rejected at config load).
- Reproducible Unix/Windows official-conformance runners pinned to
  final specification commit
  `5f5440bb26a62e2cf3440b92da5a667efa03b267` and conformance commit
  `49103de6ed70804e940637bf3e9e29e4a3f54e64`, covering request metadata,
  `tools_call`, standard/custom HTTP headers, and invalid tool headers. The
  latter is the latest official harness at verification, but it still marks `2026-07-28`
  DRAFT/provisional and vendors the pre-final schema from specification commit
  `71e306956a4959c9655e5036be215d41986596e6`; the machine-checked final
  tag/schema comparison is the separate final-spec reconciliation proof. The
  runners verify that the final tag resolves to the reviewed commit, fail if
  upstream conformance `main` no longer equals the reviewed pin, and retain
  `FINAL_SCHEMA_RECONCILIATION.txt`, the upstream status, complete official
  client inventory, and an executed/not-executed scope manifest. A passing
  retained run is a
  five-scenario tools/discovery/metadata/header gate; it is not a
  full-protocol, auth, MRTR/request-state, `subscriptions/listen`,
  schema-reference, server, authorization-server, or final-promoted-suite
  conformance claim (ADR 0023).
- `Transport::raw_send(&[u8])` — raw, unframed bytes on the wire (stdio
  writes verbatim + newline), powering the fuzzer's raw-byte payloads.
- `hang_detect` — reusable two-phase watchdog used by deadlock/race and
  selected latency-sensitive scenario paths, classifying Ok / Slow /
  Deadlock / Err (hang threshold + grace period). It is not a universal
  transport wrapper.

#### Scenarios

- `sustained` — constant load; `concurrent > 1` drives a true N-worker
  session pool (ADR 0017), with an honest, disclosed sequential fallback.
- `deadlock_probe` — the Vibe-Trading-bug-class detector; for N>1, releases
  one `hang_detect`-wrapped call per independent session through a shared
  start gate (`concurrent = 1` remains the focused single-session path).
- `cold_start` — fresh server respawned per iteration via `SessionFactory`;
  spawn-to-`initialize` handshake recorded under `cold_start:handshake`.
- `ramp` — stepped concurrency, each step its own session pool; feeds
  breaking-point analysis.
- `soak` — long-duration steady load with periodic snapshots and leak
  signals.
- `spike` — baseline, burst, cooldown; each phase its own pool with all
  burst workers joined before cooldown.
- `pattern` — multi-step weighted-random tool-call mixes with think-time and
  `ErrorBehavior`; also drives `sustained`'s multi-pattern form.
- `race_check` — N synchronized calls on independent sessions, with identical
  responses canonicalized and compared; any divergence gates the run.
- `fuzzer` — enumerated malformed-but-plausible payloads plus raw-byte
  variants (EmptyBody, InvalidJson, missing/duplicate ids, ...) with
  poisoned-session respawn between iterations.
- `version_matrix` — the same server driven once per MCP protocol revision,
  outcomes diffed side by side under per-tool metric keys `version:<rev>`
  (ADR 0018).

#### Metrics & analysis

- `Recorder` — Arc-shared atomic outcome counters plus sharded,
  lock-protected hdrhistogram latency state (p50/p95/p99/p999, microsecond
  resolution), with per-tool counters (`record_tool` /
  `snapshot_per_tool`).
- Breaking-point detector (first-violator semantics on per-step deltas) and
  A-F performance grading (worst-of-three rollup).
- Race detector (key-sorted JSON canonicalization) and coverage report
  (registered vs exercised tools, `coverage_pct`).
- Fuzz-report classification (`FuzzClass` + `has_critical` signal).
- Regression compare with configurable thresholds (ADR 0009):
  `--max-p99-regression-pct` / `--max-error-rate-regression-pp` /
  `--allow-deadlock-increase`, mirrored as `compare_runs` MCP tool args.
- Process sampling (sysinfo): RSS/CPU/fd/threads over time. Thresholds for
  p50-p999 latency, error rate, absolute memory growth (peak minus
  baseline), least-squares RSS leak slope
  (`thresholds.rss_leak_mb_per_sec`), and per-tool SLOs (`ToolSlo`).

#### Reporting

- Markdown, JSON, ANSI terminal, and self-contained HTML reporters (inline
  SVG histogram, escaped HTML, no external CDN or JS).
- `docs/schema/metrics.v1.json` — the JSON Schema for `metrics.json`, pinned
  to the JSON reporter's real output by a conformance test.
- Ratatui dashboard as a library component behind cargo feature `tui`.
  No CLI flag or subcommand exposes it in `v0.1.0`.

#### Trace record & replay (ADR 0021)

- `run --trace <file>` records every JSON-RPC frame of the run (handshake
  included) as versioned `mcp-trace/1` JSONL, with secret-looking
  `tools/call` argument values redacted by default.
- Explicit trace artifacts now finalize fail-closed: frame
  serialization/write/flush failures are latched across every session-writer
  clone and prevent a successful report or `trace_path` claim.
- `replay <trace-file> --server "cmd"` (or `--url`/`--transport`/
  `--allow-host`) re-sends the recorded client frames against a fresh server
  and diffs the responses via canonical JSON (ids ignored), exiting non-zero
  on divergence. Public `mcp_loadtest::trace` module (`TraceWriter`,
  `TracingTransport`, `ReplayReport`, `TraceError`).

#### CLI

- Subcommands: `run`, `deadlock-probe`, `cross` (N servers side by side),
  `compare` (baseline regression diff), `replay`, `doctor` (setup
  diagnostics with a per-item fix line — ADR 0014), `example-config`,
  `list-scenarios`, and `serve --mcp` — the self-hosted MCP server exposing
  `deadlock_probe` / `sustained_load` / `compare_runs` so AI agents can
  drive load tests directly.
- `--explain` on every subcommand (static algorithm text); actionable
  `Hint:` lines printed after error chains. The `run` subcommand's
  `--capture-stderr` / `--tee-stderr` flags redirect the spawned stdio
  server's initial stderr to `runs/<id>/server-stderr.log` and factory
  sessions to unique files under `runs/<id>/server-stderr/` (ADR 0013).

#### Security

- Opt-in strict schema validation (ADR 0010): `tools/call` arguments are
  validated against the server's advertised `inputSchema` (violations gate
  the run); each result's `structuredContent` is validated against the
  tool's `outputSchema` (warn-only, never gates). Schema recursion is
  depth-bounded against maliciously deep server schemas.
- SSRF defense (ADR 0012): exact-match `[server].allowed_hosts` allowlist
  plus an always-on block of private/loopback/link-local/ULA/reserved
  addresses on the http/sse/ws transports (the SSE server-provided endpoint
  URL is re-checked); redirect policy is `none` (ADR 0007).
- DNS-rebinding defense via resolver pinning (ADR 0016): hostnames are
  resolved once at connect, every resolved address is vetted against the
  blocklist, and the vetted addresses are pinned for the actual connection.
- Supply-chain gates: `cargo deny` / `cargo audit` in CI with individually
  triaged, documented advisory ignores (ADR 0011).

#### Workspace & distribution

- Six layered crates (ADR 0022), strictly downward dependencies:
  `mcp-loadtest-core` (pure data) ← `mcp-loadtest-protocol` (wire) ←
  `mcp-loadtest-engine` (scenarios + run); core ← `mcp-loadtest-output`
  (renderers + TUI); `mcp-loadtest` facade (the public API surface +
  feature-gated `serve`); `mcp-loadtest-cli` (binary). MSRV 1.88.
- Composite GitHub Action (`action.yml`) for an exact
  `uses: Teerapat-Vatpitak/mcp-loadtest@v0.1.0` pin. It installs a
  sha256-verified release binary (with `cargo install --git` fallback), runs
  `deadlock-probe`/`run`/`cross`/`doctor`, optionally compares a baseline
  `metrics.json`, and appends a job summary. External availability requires
  both the exact tag and its GitHub Release with GitHub immutable releases
  enabled; this source text alone is not availability evidence.
- Release automation is prepared for `cargo install --git` plus prebuilt
  GitHub Release binaries, subject to the hard gates in `docs/RELEASE.md`.
  The workflow refuses to publish unless GitHub immutable releases is enabled
  (verified before and immediately before publication using a separately
  provisioned read-only Administration audit token) and invokes the complete
  reusable Action contract (cross-platform argv plus composite end-to-end
  checks), not only the parser smoke test.
  crates.io publication remains a separate, irreversible maintainer decision
  and is not part of the `v0.1.0` distribution.
- Python mock-server fixtures (stdlib-only) covering
  normal/slow/broken/crash/leak/error/slow-init/malformed/schema/http/sse/
  stateless behaviors, a regression test against the real Vibe-Trading
  deadlock commit, and criterion microbenchmarks
  (record/histogram/session_loopback/hang_detect).

[Unreleased]: https://github.com/clytie0x/mcp-loadtest/compare/v0.6.0...main
[0.6.0]: https://github.com/clytie0x/mcp-loadtest/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/clytie0x/mcp-loadtest/compare/v0.4.1...v0.5.0
[0.4.1]: https://github.com/clytie0x/mcp-loadtest/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/clytie0x/mcp-loadtest/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/clytie0x/mcp-loadtest/compare/v0.2.0...v0.3.0
