# mcp-loadtest

[![CI](https://github.com/clytie0x/mcp-loadtest/actions/workflows/ci.yml/badge.svg)](https://github.com/clytie0x/mcp-loadtest/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/clytie0x/mcp-loadtest)](https://github.com/clytie0x/mcp-loadtest/releases/latest)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](#license)
[![Rust 1.88+](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](rust-toolchain.toml)

> Load tester **and bug detector** for MCP (Model Context Protocol) servers.
> Find deadlocks, hangs, response races, and performance regressions before
> they reach users.

`mcp-loadtest` drives a real MCP server through the protocol instead of calling
its internals. It is built for reliability checks that can fail CI
deterministically: no successful calls, deadlocks, response divergences,
terminal transport failures, and configured SLO violations all produce a
non-zero exit code.

## What it catches

Lazy initialization can pass `initialize` and `tools/list`, then deadlock on
the first concurrent `tools/call`. The focused probe reproduces that failure
class directly:

```bash
mcp-loadtest deadlock-probe --server "python -m my_mcp"
```

Without `--tool`, the probe calls the first tool the server marks read-only
(and not destructive) that has no required arguments, and prints its name.
Those marks are the server's own hints. If no tool qualifies, it lists the
tools and stops. Choose the tool and its arguments with `--tool` and `--args`:

```bash
mcp-loadtest deadlock-probe \
  --server "python -m my_mcp" \
  --tool get_market_data \
  --args '{"ticker":"AAPL"}'
```

```text
Status: FAIL (1 deadlock)
Deadlocks: 1   Hangs: 0   Errors: 0
Error: DEADLOCK DETECTED
```

This is the bug class behind
[HKUDS/Vibe-Trading PR #85](https://github.com/HKUDS/Vibe-Trading/pull/85).
The repository includes a
[regression test](crates/engine/tests/vibe_trading_regression.rs) pinned to the
affected revision.

Concurrent writers can also overwrite each other while every call reports
success. `lost-update` gives each write a unique marker and checks that one
read returns all of them:

```bash
mcp-loadtest lost-update \
  --server "npx -y @modelcontextprotocol/server-memory@2026.8.31" \
  --env MEMORY_FILE_PATH=/tmp/probe.jsonl --startup-timeout 60s \
  --write-tool create_entities \
  --write-args '{"entities":[{"name":"{marker}","entityType":"probe","observations":[]}]}' \
  --read-tool read_graph
```

```text
Status: FAIL (36 lost updates)
Error: lost-update failed — 36 lost updates; 53/53 successful calls; see report
```

Every call succeeded, and 36 of 50 writes were gone. See
[field results](docs/field-results.md) for this server and
mcp-knowledge-graph, and `race-check` for different answers to identical
calls.

## Install

Each [release](https://github.com/clytie0x/mcp-loadtest/releases) has a
prebuilt archive for Linux x86_64, macOS (Intel and Apple silicon), and
Windows x86_64, each with a `.sha256` file. On Linux:

```bash
V=v0.4.1
A=mcp-loadtest-$V-x86_64-unknown-linux-gnu
curl -fsSLO https://github.com/clytie0x/mcp-loadtest/releases/download/$V/$A.tar.gz
curl -fsSLO https://github.com/clytie0x/mcp-loadtest/releases/download/$V/$A.tar.gz.sha256
sha256sum -c $A.tar.gz.sha256
tar xzf $A.tar.gz
sudo install $A/mcp-loadtest /usr/local/bin/   # or copy it anywhere on your PATH
```

On macOS, set `A` to `mcp-loadtest-$V-aarch64-apple-darwin` (Apple silicon)
or `mcp-loadtest-$V-x86_64-apple-darwin` (Intel), and check with
`shasum -a 256 -c` instead of `sha256sum -c`. On Windows, download
`mcp-loadtest-<version>-x86_64-pc-windows-msvc.zip` and its `.sha256` from
the release page, and compare the hash with
`Get-FileHash <zip> -Algorithm SHA256`.

Or build the exact tag with Cargo, which takes a few minutes:

```bash
cargo install --git https://github.com/clytie0x/mcp-loadtest --tag v0.4.1 --locked mcp-loadtest-cli
```

Then check the installation:

```bash
mcp-loadtest --version
mcp-loadtest doctor
```

The project is not published on crates.io (ADR 0031).

## New in v0.4.1

- The GitHub Action downloads its binary from `clytie0x/mcp-loadtest`, the
  repository's current name, instead of the owner's old name. Move `uses:`
  pins to `@v0.4.1`.
- The Install section above gives the download and checksum commands.

v0.4.0 added:

- `deadlock-probe` without `--tool` picks a tool the server marks read-only
  that takes no required arguments, so the first probe needs only the server
  command
- `race-check` and `lost-update` subcommands run those detectors without a
  config file
- `serve --mcp` tools can target remote servers and set the target's env and
  working directory; `list_target_tools` returns each tool's annotations
- Servers started through `uvx`, `npx`, or a shell no longer outlive the
  run, and `protocol_version = "auto"` reaches servers on older Python MCP
  SDKs

Treat `v0.4.1` as published only when the exact tag, GitHub Release, platform
archives, and matching checksum sidecars all exist.

## Run a load test

Generate a starter configuration:

```bash
mcp-loadtest example-config > bench.toml
mcp-loadtest config-schema > mcp-loadtest.schema.json
```

A minimal sustained-load configuration looks like this:

```toml
[server]
command = "python"
args = ["-m", "my_mcp"]
transport = "stdio"

[scenario]
type = "sustained"
duration = "60s"
concurrent = 50
tool = "get_market_data"
args = { ticker = "AAPL" }

[thresholds]
p99_latency = "500ms"
error_rate = 0.01
hang_timeout = "5s"

[output]
report_dir = "./runs"
formats = ["terminal", "markdown", "json"]
```

Run it:

```bash
mcp-loadtest run --config bench.toml
```

Each run can produce terminal, Markdown, JSON, JUnit XML, and Prometheus text
reports.
Use the schema-stable `metrics.json` for CI comparisons:

```bash
mcp-loadtest compare \
  runs/baseline/metrics.json \
  runs/current/metrics.json
```

## Use it in GitHub Actions

Pin both the Action and binary to the immutable release:

```yaml
- name: Probe MCP server for deadlocks
  uses: clytie0x/mcp-loadtest@v0.4.1
  with:
    version: v0.4.1
    server: "python -m my_mcp"
    args: '["--tool","get_market_data","--concurrent","10","--args","{\"ticker\":\"AAPL\"}"]'
```

The Action downloads a prebuilt binary, verifies its SHA-256 checksum, and
fails the job when the command or an optional baseline comparison fails.
[mcp-loadtest-action-demo](https://github.com/clytie0x/mcp-loadtest-action-demo)
runs it against a healthy server, which must pass, and a deadlocking one,
which must fail.
Its `args` input is a JSON array of literal arguments; shell syntax is not
evaluated. See the complete
[CI integration guide](docs/examples/ci-integration.md) for reports,
thresholds, and baseline gating.

## Use it from an agent

`mcp-loadtest serve --mcp` is itself an MCP server. Register it with your
agent once, then ask it to probe another server. In Claude Code:

```bash
claude mcp add mcp-loadtest -- mcp-loadtest serve --mcp
```

For clients configured with an `mcpServers` JSON file:

```json
{
  "mcpServers": {
    "mcp-loadtest": { "command": "mcp-loadtest", "args": ["serve", "--mcp"] }
  }
}
```

Then ask, for example: "Use mcp-loadtest to check `python -m my_mcp` for
deadlocks and lost updates." The server tells the agent which tool answers
which question and in what order to call them. A typical session calls
`list_target_tools`, then `cold_start`, `deadlock_probe`, `race_check`, and
`lost_update`, and branches on each result's `passed` and
`failure_reasons`. Targets are a stdio command (`server_command`) or a
remote endpoint (`server_url`). See
[docs/examples/lost-update.md](docs/examples/lost-update.md#from-an-agent)
for a full call.

Run tools start real server processes and make real tool calls. Point them
at servers and data the agent may exercise.

## What it covers

- **Correctness under concurrency** — synchronized deadlock and race probes
  across independent sessions.
- **Load baselines** — sustained, cold-start, and weighted multi-step
  workloads that share the detectors' session pool and supply the latency
  baseline that separates a hang from a slow server.
- **MCP-aware metrics** — p50/p95/p99/p999 latency, throughput, error classes,
  process sampling, coverage, and per-tool SLOs.
- **Record and replay** — capture versioned JSONL traces, replay them against a
  fresh server, and fail on response divergence.
- **Regression analysis** — compare two `metrics.json` reports and gate on
  p99, error-rate, and deadlock regressions.
- **CI and observability** — JSON, JUnit, and Prometheus outputs.
- **Agent access** — `mcp-loadtest serve --mcp` lets an agent list a target's
  tools, run `deadlock_probe`, `race_check`, `lost_update`, `cold_start`, and
  `sustained_load`, and compare runs. Every run tool returns `passed` plus
  machine-readable `failure_reasons` and the `run_dir` of its artifacts.

## Built-in scenarios

| Scenario | Purpose |
| --- | --- |
| `cold_start` | Measure spawn, initialization, and first-call latency |
| `sustained` | Track steady-state latency, throughput, and errors |
| `deadlock_probe` | Reproduce lazy-init and first-call deadlocks |
| `race_check` | Detect non-deterministic responses |
| `lost_update` | Detect acknowledged writes that concurrent writers lose |
| `pattern` | Run weighted multi-step tool-call sequences |
| `version_matrix` | Compare behavior across MCP protocol revisions |

Run `mcp-loadtest list-scenarios` for the CLI summary. Configuration details
and design rationale live in [DESIGN.md](DESIGN.md).

## Transports and security

The client supports `stdio`, Streamable HTTP, and SSE transports.
Remote targets require an explicit host allowlist. Credentials are read from
environment variables through `headers_from_env`; literal remote credentials
in TOML and URL userinfo are rejected. Credential-bearing remote connections
require HTTPS.

For OAuth, `[server.auth]` supports authorization-code + PKCE and
client-credentials flows. Discovery, refresh, and bounded scope step-up are
handled automatically; client secrets are referenced by environment-variable
name.

See the [transport security ADR](docs/adr/0007-transport-security-posture.md)
for resolver pinning, header restrictions, redaction boundaries, and response
size limits.

## Protocol compatibility

`protocol_version = "auto"` discovers the compatible MCP era and falls back
only when the peer supplies protocol evidence. A stdio server that exits on
the discovery probe, as servers on older Python MCP SDKs do, is restarted
once with the legacy handshake
([ADR 0029](docs/adr/0029-restart-stdio-server-killed-by-discovery.md)).
The final `2026-07-28`
client-role surface is supported, while `2025-11-25`, `2025-06-18`, and
`2025-03-26` remain available for pinned compatibility testing. The scoped
official client conformance matrix is documented in
[ADR 0023](docs/adr/0023-mcp-2026-final-reconciliation.md).

## Documentation

- [Cookbook examples](docs/examples/) — CI integration, custom scenarios,
  deadlock debugging, and finding lost updates
- [DESIGN.md](DESIGN.md) — architecture, behavior, and product positioning
- [Architecture decisions](docs/adr/) — security and implementation rationale
- [Metrics schema](docs/schema/metrics.v1.json) — versioned JSON report contract
- [Config schema](docs/schema/config.v1.json) — Draft 2020-12 editor contract
- [CHANGELOG.md](CHANGELOG.md) — release history
- [SECURITY.md](SECURITY.md) — vulnerability reporting and security posture

## Development

```bash
git clone https://github.com/clytie0x/mcp-loadtest
cd mcp-loadtest
bash scripts/ci-checks.sh
# Windows: pwsh scripts/ci-checks.ps1
```

Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request.

## Release status

The source version is `0.4.1`, but source text alone does not prove release
availability. Consider `v0.4.1` published only after its tag, checksums,
platform assets, and GitHub Release are verified. The immutable `v0.1.0`
release remains available under `MIT OR Apache-2.0`; later development is
Apache-2.0.

## License

Development after `v0.1.0` is licensed under the
[Apache License 2.0](LICENSE). The immutable `v0.1.0` release retains its
original `MIT OR Apache-2.0` terms.
