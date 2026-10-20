# 35. `lost-update` picks its write and read tools when they are omitted

Date: 2026-10-20
Status: Accepted

## Context

`lost-update` needs `--write-tool`, `--write-args` with `{marker}` in a
string, and `--read-tool`. Writing `--write-args` means reading the write
tool's input schema first. For server-memory that is
`{"entities":[{"name":"{marker}","entityType":"probe","observations":[]}]}`.
This is the largest barrier for a maintainer trying the check on their own
server. ADR [0030](0030-deadlock-probe-picks-a-tool.md) removed the same kind
of step from `deadlock-probe` by choosing a tool from the server's
annotations.

A lost-update choice carries more risk than a deadlock-probe choice. The
write tool is called with real writes, dozens of times, so a wrong pick
changes data. The read tool decides the verdict: a read that lists only
part of the store (a page, the newest N, a summary) would report writes it
merely did not show as lost. A false "lost updates" FAIL is the worst
outcome for a CI gate, so a wrong pick must end with a message or an
inconclusive run instead.

Annotations are still the server's own hints, not guarantees (ADR 0030).

## Decision

`--write-tool`, `--write-args` and `--read-tool` become optional. When any
of them is missing, one session lists every `tools/list` page with the
check's server command, environment and startup timeout, and is shut down
before the check starts. Then only the missing parts are filled in. Each
choice is printed to stderr with its reason, followed by "Writes are real:
point the server at a disposable store."

**Partial flags are allowed.** What the user names is used as given:

- `--write-tool` without `--write-args`: arguments are built from that
  tool's schema, whatever its annotations. Naming the tool is the user's
  choice. If the server does not list it, or its schema cannot be filled,
  the check stops and asks for `--write-args`.
- `--write-args` without `--write-tool`, or `--read-args` without
  `--read-tool`, is a usage error, as `--args` without `--tool` is.

**Write tool.** The first listed tool that meets all of these:

- `readOnlyHint` is not `true`;
- `destructiveHint` is explicitly `false`. MCP's default for an absent
  `destructiveHint` is `true`, so absence does not count;
- `openWorldHint` is explicitly `false`, for the same reason. A write to an
  open world (a message, an email, an issue) must never be repeated 50
  times by default;
- its arguments can be built (below).

Names starting with `create`, `add`, `append`, `insert`, `write`, `save`,
`record`, `store`, `put` or `push` come first, then listed order. A tool
that updates a record (`update_entry`) would be refused for a record that
does not exist, which leaves the check inconclusive.

**Write arguments.** Built from `inputSchema`, filling required properties
only:

- every string gets `{marker}`, so names and IDs stay unique per write;
- `const` and `enum` take their value or first value;
- integers and numbers take the smallest value within `minimum` /
  `exclusiveMinimum` / `maximum` / `exclusiveMaximum` that is non-negative
  if possible, or their `default`; an integer `multipleOf` moves that value
  to the next multiple; booleans take their `default` or `false`;
- an array gets one element built from `items`. If that element cannot be
  built and `minItems` allows it, the array is left empty;
- objects recurse, and local `$ref`s (`#/...`) are followed, up to 8 levels.
  A `$ref` with an assertion beside it (a `pattern`, a `type` ...) is
  refused rather than merged.

It refuses, naming the property, when it cannot fill a schema with
confidence: `anyOf`, `oneOf`, `allOf`, `not`, `if`; a string with a
`format`, a `pattern`, or a length that rules out a 22-character marker; an
array that needs two or more items; tuples; a required property without a
schema or type; a `multipleOf` it cannot meet. It also refuses when no
required string is left to carry `{marker}`. server-memory's
`create_entities` becomes
`{"entities":[{"name":"{marker}","entityType":"{marker}","observations":["{marker}"]}]}`
and mcp-taskmanager's `request_planning` becomes
`{"originalRequest":"{marker}","tasks":[{"title":"{marker}","description":"{marker}"}]}`.

**Read tool.** The first listed tool that the server marks
`readOnlyHint: true` and not `destructiveHint: true`, and that takes no
arguments at all: no `required`, no `properties`, and nothing else in the
schema that could require one (`$ref`, `allOf`, `if` and the like). An
optional `limit` or
`page` could make the read list only part of the store, so even optional
arguments disqualify the tool. Names starting with `read`, `list`, `dump`,
`export` or `get_all` come first, so `read_graph` wins over `get_status`.

**A picked read confirms every loss.** The scenario gains
`recheck_losses`, which the CLI sets only when it picked the read tool.
When the final read is missing markers, the check makes 3 more writes one
at a time and reads again. The loss counts only if that second read shows
all 3 new markers, every marker the first read showed and every other
entry it showed (a line, or a string in JSON, with digit runs compared
equal so a count or timestamp may move), and still lacks the missing ones. Otherwise the run is inconclusive, with a note to name a
read tool. This catches a read that caps its output at the newest N items
(the new writes push out markers or other data it showed before), at the
first N (the new
writes do not show), or truncates (the new writes are at the end). It also
catches a store whose missing writes appear later, which is not consistent
yet rather than lossy. A loss on a store that holds its data is confirmed
without changing the verdict: sequential writes by one client do not race.

**Nothing qualifies.** The check exits with the CLI's error code (1)
before any write. It lists the server's tools (up to 20) with the reason
each is not a write tool or not a read tool, for the roles that were
missing, and the hint to pass `--write-tool` and `--read-tool`.

## Alternatives considered

- **All or none of the three flags.** Simpler to explain, but it throws away
  the most useful partial case: a server with no annotations (mcp-taskmanager)
  where the user knows the tool names but not the argument shape.
  `--write-tool request_planning --read-tool list_requests` is enough there.
- **Treat an absent `destructiveHint` or `openWorldHint` as `false`.** That
  would pick from unannotated servers, which are most of them today, but it
  reverses MCP's defaults and would call any unannotated tool that takes a
  string, delete and send tools included. Rejected.
- **Put the marker in one string only.** A second required string such as
  an ID or a name would then repeat across writes, and a store that rejects
  or merges duplicates would look lossy. Every string gets the marker.
- **Check the read's capacity before the race** by writing as many markers
  one at a time as the run will write. It costs that many writes on every
  run and still misses a read that shows the oldest N items, because the
  concurrent writes come after the calibration ones. Confirming only when a
  loss is seen costs 4 calls, and only on that path.
- **Recheck every run, not only a picked read.** A store that keeps only
  the last write (mock-store's `last` mode) loses data even sequentially,
  and is a real total loss that the default check reports. A recheck makes
  it inconclusive, because it cannot be told apart from a read capped at
  one item. A read the user names is one the user vouches for.

## Consequences

- `lost-update --server "<cmd>"` reaches a verdict on servers that annotate
  a closed-world, non-destructive write and a read-only listing without
  arguments, such as server-memory.
- The pick relies on the server's hints. A tool misannotated as
  non-destructive and closed-world would be called with real writes. This is
  mitigated by requiring both hints explicitly, printing the choice and the
  arguments before the run, and the documented requirement that the store be
  disposable.
- Servers that annotate nothing (mcp-taskmanager 1.1.1) get the list and
  the hint. Naming the two tools is enough; the arguments are still built.
- Name prefixes rank qualifying tools; they never make a tool qualify.
- One extra server start (the discovery session) per run that picks or
  builds anything.
- A loss found through a picked read costs 4 more calls, and a read that
  does not list everything ends inconclusive rather than as a pass or a
  fail. A real loss behind such a read needs `--read-tool` to be reported.
- `run` configs and the `serve --mcp` `lost_update` tool still take all
  three explicitly; they can adopt the picker later.
