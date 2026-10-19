# 30. `deadlock-probe` picks a read-only tool when `--tool` is omitted

Date: 2026-10-19
Status: Accepted

## Context

Phase 3 of the roadmap asks for a one-command quickstart that probes a
server without a config file. `deadlock-probe --server "<cmd>"` was meant to
be that command, but `--tool` defaulted to `echo`, which most real servers
do not have. A new user first had to find a tool name (with another client,
or `serve --mcp`'s `list_target_tools`) and then run the probe: two steps,
and the first one is outside this tool.

The server already says which of its tools are safe to call repeatedly, in
the MCP tool annotations. Those annotations are hints from the server, not
guarantees: the MCP specification says clients must treat annotations as
untrusted unless they come from a trusted server, and `list_target_tools`
already describes them as "the target's own hints, not guarantees". Here the
user chose the server by naming its command, but the probe cannot know
whether its annotations are accurate.

## Decision

`--tool` has no default. When it is given, the probe behaves as before.

When it is omitted, `deadlock-probe`:

1. Opens one session with the same server command, environment, and startup
   timeout the probe uses, reads every `tools/list` page (following
   `nextCursor`), and shuts that session down through the shared teardown
   helper before the probe starts.
2. Picks the first tool, in the server's listed order, that meets all of:
   - `annotations.readOnlyHint` is `true` (absent does not count);
   - `annotations.destructiveHint` is not `true`;
   - `inputSchema` has no `required` properties (missing or empty).
3. Calls it with `{}` and prints one line to stderr naming the tool and
   saying the choice rests on the server's own hints, and that `--tool`
   chooses.
4. If no tool qualifies, exits with the CLI's error exit code (1, as for bad
   `--args` JSON) after listing the server's tools (name, read-only or not,
   destructive, required argument names) and the hint
   `pass --tool NAME and --args '{...}'`.

`--args` without `--tool` is a usage error: arguments for a tool the user
did not name make no sense.

No other signal is used. In particular, tool names and descriptions are
ignored.

## Alternatives considered

- **A new `check` subcommand that also runs `race-check`.** One command for
  several detectors is attractive, but `race-check` on read-only tools fails
  falsely on any tool whose answer changes over time (a clock, a status
  counter, a random sample), and those are exactly the read-only, no-argument
  tools this rule picks. It also adds API surface to maintain. Rejected for
  now; it can be revisited with a way to exclude time-varying tools.
- **Document the two-step quickstart only.** No code, but it keeps the two
  steps the roadmap item exists to remove. Rejected.

## Consequences

- `deadlock-probe --server "<cmd>"` reaches a verdict on any server that
  annotates at least one read-only, no-argument tool.
- The choice relies on the server's hints. A misannotated destructive tool
  with no required arguments would be called five times at once (the quick
  default `--concurrent`). This is mitigated by requiring an explicit
  `readOnlyHint: true`, rejecting `destructiveHint: true`, and printing the
  choice before the probe runs; the README and `--explain` say the marks are
  the server's own.
- Users who relied on the implicit `echo` must now pass `--tool echo`. On a
  server whose `echo` is unannotated, the probe now stops with the tool list
  instead of probing `echo`.
- Servers that annotate nothing get the list and the hint, not a probe.
- One extra server start (the discovery session) per run without `--tool`.
  In the composite Action's redaction mode its stderr is discarded and its
  errors are made generic, like the probe's own sessions.
