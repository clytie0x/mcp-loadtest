# 34. Write an official-SDK repro script from a pipelined lost-update finding

Date: 2026-10-20
Status: Accepted

## Context

A `lost_update` finding is only useful once the server's maintainers
accept it, and they will not install mcp-loadtest to check it. Every
upstream report so far (issue #36) carried a script written by hand on the
official MCP SDK: start the server over stdio, send a few writes at once
with `Promise.all` or `asyncio.gather`, each with a unique value, read once,
and print how many survived. Writing it took longer than finding the bug,
and each one restated what the run already knew: the server command, its
env, the write and read tools and their arguments.

All of those scripts reproduce the pipelined case (ADR
[0033](0033-pipelined-lost-update.md)): one client, one server process,
tool calls sent without waiting for each reply. That is what a client
running tool calls in parallel does, and it needs nothing shared between
processes, so it is the case maintainers can run and fix first.

## Decision

Add `--emit-repro <path>` to `lost-update` and `run`, and generate the
script from the run's own plan.

- **Pipelined only.** `lost-update --emit-repro` requires `--pipelined`, and
  `run --emit-repro` requires a `lost_update` config with `pipelined = true`
  against a stdio `command`. Anything else is refused before the run
  starts. A script for the multi-process case would have to start and
  coordinate several clients, and would no longer look like the minimal
  repro maintainers expect.
- **Written only for a finding.** The script is written when the run
  reports lost updates, and the CLI prints its path and how to run it. A
  passing run writes nothing and says so, so a script on disk always
  stands for a loss that was observed.
- **Two SDKs, chosen by extension.** `.mjs` produces an ES module
  on `@modelcontextprotocol/sdk` (`Client`, `StdioClientTransport`); `.py`
  produces a script on the `mcp` package (`ClientSession`,
  `StdioServerParameters`, `stdio_client`). `--repro-sdk node|python`
  overrides the extension.
- **The same server and calls.** Command, args, env and working directory
  come from the run. The env is passed as `{ ...process.env, ... }` or
  `{**os.environ, ...}`, as the hand-written repros do, because the SDKs
  otherwise start the server with a reduced environment. The script sends
  one round of `concurrent` writes on one session, then the read with
  `read_args`. Markers keep the engine's format, with a fresh nonce per run.
  `{worker}` is the call's index and `{seq}` is `0`.
- **A verdict, not a dump.** The script counts each write as acknowledged,
  refused (`isError`) or failed, as the engine does, prints how many
  acknowledged markers the read shows, and exits 1 if any is missing.
- **Values are embedded as literals, never interpolated.** Every string
  from the plan becomes a double-quoted literal that is valid in both
  JavaScript and Python and is printable ASCII: quotes and backslashes are
  escaped and everything else becomes a `\u` escape, so no value can end a
  string or a comment. `write_args` becomes an object literal in the target
  language, with a placeholder-bearing string written as a concatenation of
  literals and the script's variables (`"note-" + m`). A JavaScript
  `__proto__` key is written computed, so it stays a field.

## Alternatives considered

- **Embed the arguments as a JSON string and substitute at run time**
  (`JSON.parse(...)` plus a recursive replace). Just as safe, but the
  arguments become unreadable, and a maintainer reading the issue should
  see the call as plainly as in a hand-written repro.
- **Precompute every call's arguments.** Simple, but `concurrent` copies of
  the arguments bury the one line that matters.
- **Always write the script, before the run.** It would exist even when the
  server is fine, and a passing script attached to a report misleads.
- **A `serve --mcp` tool argument.** An agent can already turn the run's
  inputs into a script; the CLI is where a person filing a report starts.
  It can be added later without changing the generator.

## Consequences

- The script hard-codes the run's env and paths, secrets included. The
  header and the docs say to check it before posting it.
- A loss seen only across processes has no generated repro.
- The script follows the SDKs' public client API. A breaking SDK release
  needs the templates updated; the Python script already reads both
  `is_error` (mcp 2.x) and `isError` (1.x).
- Two ignored integration tests run generated scripts on both SDKs against
  `mock-store.py`, and need Node.js or a Python venv and the network.
