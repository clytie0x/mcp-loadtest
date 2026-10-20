# Draft: concurrency guidance for MCP tool calls

Status of this file: **draft, not submitted.** It is written for the project
owner to submit personally; see [README.md](README.md). Nothing below has been
posted anywhere.

This file has three parts:

1. [Research notes](#part-1-research-notes): what the specification, the
   server guide and the SDK docs say today, with sources, and which existing
   issues and proposals are related.
2. [Venue and submission plan](#part-2-venue-and-submission-plan).
3. [The SEP draft](#part-3-sep-draft), in the format of
   [`seps/TEMPLATE.md`](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/seps/TEMPLATE.md),
   followed by [Appendix A](#appendix-a-companion-docs-text-no-sep-needed),
   the shorter text for a docs-only pull request.

Part 3 is the text to copy into `seps/0000-concurrent-tool-calls.md` in the
specification repository. Parts 1 and 2 are for the owner and are not part of
the submission. Research was done on 2026-10-20 against these revisions:
specification repository `1155ca4` (2026-10-17), TypeScript SDK `46d9c99`,
Python SDK `2af9e30`.

---

## Part 1: Research notes

### The assumption holds: nothing tells server authors that tool handlers run concurrently

The latest revision is 2026-07-28
([modelcontextprotocol.io/specification/latest](https://modelcontextprotocol.io/specification/latest)
resolves to it). A search of the 2026-07-28, 2025-11-25 and draft
specification text for concurrency, parallel, ordering, serialization, race,
lock and idempotency finds the following, and nothing else relevant.

| Topic | What the spec says | Source |
|---|---|---|
| Request IDs | "The request ID **MUST NOT** match the ID of any other request the sender has issued and not yet received a response for." This implies several requests may be outstanding at once, but it is the only such hint. | [basic/index, Requests](https://modelcontextprotocol.io/specification/2026-07-28/basic#requests) |
| Ordering | No statement that responses may arrive in any order or that the server may process requests concurrently. The only ordering rules are for `tools/list` (deterministic order) and for subscription notifications (per subscription ID). | [server/tools](https://modelcontextprotocol.io/specification/2026-07-28/server/tools), [basic/patterns/subscriptions](https://modelcontextprotocol.io/specification/2026-07-28/basic/patterns/subscriptions) |
| Parallel tool use | Mentioned only for **sampling**: "MCP allows models to make multiple tool use requests in parallel", and "Provider APIs can concurrently process multiple tool uses". This is the server asking the client's model; it says nothing about how a server should handle parallel `tools/call` requests. Same text in 2025-11-25. | [client/sampling, Parallel Tool Use](https://modelcontextprotocol.io/specification/2026-07-28/client/sampling) |
| Statelessness | "A server processes each request independently"; "Servers **SHOULD NOT** require that a client reuse the same connection or process to perform related operations." So related calls may arrive on different connections or reach different server processes, which share whatever store the server keeps. The text does not mention concurrent access to that store. | [basic/index, Statelessness](https://modelcontextprotocol.io/specification/2026-07-28/basic#statelessness) |
| Stdio | All messages share one channel, responses are "correlated by JSON-RPC `id`". On a crash, "in-flight requests are simply lost and the client can retry them against the fresh process". | [basic/transports/stdio](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio) |
| Cancellation | "Both parties **MUST** handle these race conditions gracefully", about a cancel arriving after the work finished. This is the only use of "race" in the base protocol. | [basic/patterns/cancellation](https://modelcontextprotocol.io/specification/2026-07-28/basic/patterns/cancellation) |
| Stateful tools | Non-normative section: servers keep state behind an explicit handle (`create_basket` returns `basket_id`, then `add_item` takes it). Discusses authorization, opacity, lifetime and expiry of handles. Does not mention two `add_item` calls on one basket at the same time, which is exactly the read-modify-write case. | [server/tools, Stateful Tools](https://modelcontextprotocol.io/specification/2026-07-28/server/tools#stateful-tools) |
| Tool annotations | `readOnlyHint`, `destructiveHint` (default true), `idempotentHint` ("calling the tool repeatedly with the same arguments will have no additional effect", default false), `openWorldHint`. All are hints, and "clients **MUST** consider tool annotations to be untrusted unless they come from trusted servers". None concerns concurrent calls. | [schema.ts `ToolAnnotations`](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/1155ca460e5097019f32769b38dcba194432e2fb/schema/2026-07-28/schema.ts), [server/tools, Tool](https://modelcontextprotocol.io/specification/2026-07-28/server/tools#tool) |
| Security considerations for tools | Servers MUST validate inputs, implement access controls, rate limit, sanitize outputs. Nothing on concurrent access. | [server/tools, Security Considerations](https://modelcontextprotocol.io/specification/2026-07-28/server/tools#security-considerations) |
| Tasks (2025-11-25) | Limits on "concurrent tasks per requestor"; about resource limits, not shared state. | [2025-11-25 basic/utilities/tasks](https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/tasks) |

Server-building documentation:

- The [Build an MCP server](https://modelcontextprotocol.io/docs/develop/build-server)
  guide (2026-07-28 docs, all languages) has a "Best Practices" section about
  logging to stderr only. It does not mention concurrency or shared state.
- [Server concepts](https://modelcontextprotocol.io/docs/learn/server-concepts)
  does not either.
- The Inspector [recipes](https://modelcontextprotocol.io/docs/tools/inspector/recipes)
  page lists "concurrent calls" among the edges to test, without saying what
  correct behaviour is.

SDKs:

- **TypeScript SDK.** Neither the [README](https://github.com/modelcontextprotocol/typescript-sdk/blob/46d9c9921b6542e813c1ecc5f9fc964085e8e3bb/README.md)
  nor `docs/servers/tools.md` says that handlers can run concurrently. The
  code does run them concurrently: `Protocol._onrequest` starts each handler
  with `Promise.resolve().then(() => handler(request, ctx))` and does not wait
  for earlier ones, so two `async` handlers interleave at every `await`
  ([protocol.ts](https://github.com/modelcontextprotocol/typescript-sdk/blob/46d9c9921b6542e813c1ecc5f9fc964085e8e3bb/packages/core-internal/src/shared/protocol.ts#L1113-L1115)).
- **Python SDK.** The only statement in any SDK doc is in the v2
  [migration guide](https://py.sdk.modelcontextprotocol.io/migration/#sync-handler-functions-now-run-on-a-worker-thread):
  "Most servers simply gain concurrency. [...] Synchronous handlers can run
  concurrently with each other, up to anyio's default worker-thread limit."
  [Tools](https://py.sdk.modelcontextprotocol.io/servers/tools/) says only
  that a plain `def` tool runs in a thread "so it never blocks the server".
  `async def` handlers are not described as concurrent, though they are.
  Neither page says that state shared between handlers needs a lock.

### Related issues, proposals and groups

No existing issue, discussion or SEP proposes guidance on concurrent tool
calls that modify shared state. The related ones:

| Item | Relation | State (2026-10-20) |
|---|---|---|
| [#3394](https://github.com/modelcontextprotocol/modelcontextprotocol/issues/3394) "Lost-response retry on a mutating `tools/call` re-executes external effects" | Closest. About **sequential retries** after a lost response (duplicate effects), not concurrent calls (lost effects). Its thread favours non-normative text in `server/tools` ("The protocol defines no idempotency, duplicate-detection, or exactly-once semantics for `tools/call`") plus a later retry-identity SEP. The same section would hold both texts. No maintainer reply yet. | Open, active |
| [PR #3182](https://github.com/modelcontextprotocol/modelcontextprotocol/pull/3182) SEP-3182 "Request Idempotency" | Idempotency keys for retries. Closed on 2026-08-23 under the AI-disclosure policy, not on its merits. | Closed |
| [Filesystems WG charter](https://modelcontextprotocol.io/community/working-groups/filesystems) | Will specify "how two writers avoid a lost update" for **resource writes** (optimistic concurrency control). Same failure, different primitive: it does not cover tools. | Chartered 2026-08-24 |
| [Tool Annotations IG](https://modelcontextprotocol.io/community/interest-groups/tool-annotations) | Reviews all annotation proposals. None of its listed SEPs (1862, 1913, 1984, 2417) proposes a concurrency hint; [SEP-1984](https://github.com/modelcontextprotocol/modelcontextprotocol/pull/1984) proposed six hints, none about concurrency, and was closed to be synced with the IG. | Active |
| [Discussion #1525](https://github.com/modelcontextprotocol/modelcontextprotocol/discussions/1525) "How to handle parallel tool calls in custom mcp client?" | A community reply says "The server implementation and downstream system must also support concurrency", with no spec text to cite. No maintainer answer. | Closed |
| [Discussion #938](https://github.com/modelcontextprotocol/modelcontextprotocol/discussions/938) "Some ideas about concurrent and asynchronous MCP interfaces" | About concurrent task structures, not shared state. | Closed |
| [#3435](https://github.com/modelcontextprotocol/modelcontextprotocol/issues/3435) "measured dispatch-overlap gap" | About a guardrail check not being atomic with dispatch. Not shared state. | Closed |

TypeScript and Python SDK issue searches for "concurrent tool calls" (46 and
57 results) found transport and request-ID races, but nothing on handler
state.

### How SEPs are submitted

From the [SEP guidelines](https://modelcontextprotocol.io/community/sep-guidelines),
[CONTRIBUTING.md](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/CONTRIBUTING.md),
[communication](https://modelcontextprotocol.io/community/communication) and
[AI_POLICY.md](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/AI_POLICY.md):

- **When a SEP is needed.** New or changed protocol behaviour needs one.
  "Documentation clarifications" and "Adding examples to existing features"
  skip the SEP process and go straight to a pull request.
- **Prior discussion is required.** "A SEP without a linked prior discussion
  is not accepted." Discuss it in the relevant working or interest group on
  Discord, and link the Discord thread, meeting notes or GitHub Discussion in
  the PR description. Since 2026-09-25, GitHub Discussions in the spec
  repository take only meeting notes. Discord decisions must be moved to a
  GitHub issue, and #3394's author asked for an issue to count as the linked
  discussion.
- **File and PR.** Name the file `seps/0000-title.md` from the template, open
  a PR, then rename the file to the PR number. The guidelines say "The
  repository accepts pull requests from collaborators only"; CONTRIBUTING.md
  describes the usual fork-and-PR flow. Check which applies before starting.
- **Sponsor.** Tag one or two maintainers from
  [MAINTAINERS.md](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/MAINTAINERS.md)
  whose area fits. Without a sponsor in 6 months the SEP becomes `dormant`.
- **Acceptance.** Requires a prototype. A Standards Track SEP with observable
  behaviour needs a conformance scenario before `Final`. "Implementation-hardening
  recommendations" are exempt.
- **AI disclosure.** Any AI assistance must be disclosed in the PR or issue,
  with its extent. SEP-3182 was closed for not doing so.

---

## Part 2: Venue and submission plan

**Choice: both, in this order.**

1. **Prior discussion first (required for the SEP).** Open one GitHub issue
   in the spec repository with the problem and the evidence (the Motivation
   section below, shortened). Post a link in Discord in the
   [Filesystems WG](https://modelcontextprotocol.io/community/working-groups/filesystems)
   channel (`#filesystems-wg`), which already owns "how two writers avoid a
   lost update" for resources and is the group most likely to care. If the
   annotation question in Open Questions is pursued, also post in the
   [Tool Annotations IG](https://modelcontextprotocol.io/community/interest-groups/tool-annotations)
   channel. Optionally add a short cross-link on #3394, since both texts
   belong in the same `server/tools` section. Do not merge the two topics:
   #3394 is about duplicate effects from retries, this is about lost effects
   from concurrent calls.
2. **Docs PR (no SEP needed).** [Appendix A](#appendix-a-companion-docs-text-no-sep-needed)
   adds a short "Concurrent tool calls" section to the server guide. It only
   explains existing behaviour, so it falls under "documentation
   clarifications". It can land before or without the SEP, and is the
   cheapest change that reaches server authors. The same paragraph fits the
   TypeScript SDK's `docs/servers/tools.md` and the Python SDK's
   `docs/servers/tools.md`, as separate PRs in those repositories.
3. **SEP (for the normative text).** [Part 3](#part-3-sep-draft). The
   normative sentences (SHOULD assume concurrency, MUST NOT silently lose an
   acknowledged write) change what a conforming server must do, so they need
   a SEP, not a docs PR. Type: Standards Track. It adds no message, field or
   capability, but the MUST NOT changes observable behaviour (conflicting
   successful calls must keep their effects), so it is not exempt from the
   conformance-test requirement: the SEP plans a conformance scenario and its
   traceability mapping (see the Testing Plan). If maintainers
   object to new MUST-level text, it can become an Informational SEP with the
   same content at SHOULD level.

**Why not only a docs PR:** advice in a guide does not change what "correct"
means. Server authors read the spec's `server/tools` page. Today it describes
handles for state shared across calls without saying the calls may overlap.
**Why not only a SEP:** a SEP takes months and needs a sponsor; the guide text
helps server authors now.

**Before submitting, the owner should:**

- Decide whether to report the open server bugs upstream first. Four
  (server-filesystem, mcp-taskmanager, mcp-shrimp-task-manager, mnemonic-mcp)
  are verified but not reported
  ([field-results.md, Upstream check](../field-results.md#upstream-check-2026-10-19)).
  A public SEP naming them before their maintainers hear about it would be
  unfriendly. Reporting first, or naming servers only by class in the SEP,
  both avoid that.
- Add the AI disclosure required by AI_POLICY.md (a suggested sentence is in
  the SEP preamble) and fill in author, sponsor and PR number.
- Re-read the evidence numbers against the current
  [field-results.md](../field-results.md); they are copied as of 2026-10-20.

---

## Part 3: SEP draft

Everything from here to the end of the file, apart from Appendix A, is the
SEP text.

# SEP-0000: Concurrent tool calls and shared state

- **Status**: Draft
- **Type**: Standards Track
- **Created**: 2026-10-20
- **Author(s)**: {Name} <{email}> (@{github-username})
- **Sponsor**: None (seeking sponsor)
- **PR**: https://github.com/modelcontextprotocol/modelcontextprotocol/pull/{NUMBER}
- **Prior discussion**: {link to the issue or Discord thread}
- **AI disclosure**: {e.g. "The first draft of this SEP and the research behind
  it were written with Claude Code. The evidence comes from runs of
  mcp-loadtest that I made and checked myself; I have reviewed and edited the
  text."}

## Abstract

A client may send a `tools/call` before earlier ones have answered, and
models routinely issue parallel tool calls. Separately, several clients or
server processes may share one store. The specification says neither of
these to server authors. It does not say that tool handlers may run
concurrently, or what a server owes a caller when two calls modify the same
state. As a result, published servers load a file or an in-memory structure,
modify it and write it back with no lock. Every call returns success, and all
but one of the concurrent writes is lost.

This SEP adds a short "Concurrent Tool Calls" section to `server/tools`:
servers SHOULD assume tool calls can run concurrently, MUST NOT report success
for a modification whose effect a concurrent call then silently loses (the
outcome must match some sequential order of the successful calls), and SHOULD
either serialize conflicting modifications or refuse them with a tool
execution error. Clients get one sentence: a client that needs one call to
see another's effect waits for the first response. The SEP adds no messages,
fields, capabilities or annotations, and it states what `idempotentHint` does
not promise.

## Motivation

### The specification is silent where server authors need it

- The base protocol allows several outstanding requests (a request ID "MUST
  NOT match the ID of any other request the sender has issued and not yet
  received a response for"), but never says the receiver may process them
  concurrently or answer them in any order.
- "Parallel Tool Use" appears only in `client/sampling`, about the server's
  sampling request to the client's model.
- The 2026-07-28 statelessness rules say servers "SHOULD NOT require that a
  client reuse the same connection or process to perform related
  operations". Related calls can therefore reach different processes that
  share one store, and the spec gives no guidance on that store.
- The non-normative "Stateful Tools" section shows `create_basket` and
  `add_item` on a shared `basket_id`. It covers authorization, opacity,
  lifetime and expiry of the handle, but not two `add_item` calls on the same
  basket at once, which is the case that loses data.
- `idempotentHint` covers repeating a call with the **same** arguments. It
  says nothing about concurrent calls with **different** arguments, which is
  where writes are lost.

The official SDKs dispatch requests concurrently. The TypeScript SDK starts
each handler without waiting for earlier ones, and the Python SDK v2 runs
synchronous handlers on worker threads. Only the Python migration guide
mentions it ("Most servers simply gain concurrency"). The server-building
guide and the SDK tool pages do not.

### Evidence: acknowledged writes are lost in published servers

The evidence comes from [mcp-loadtest](https://github.com/clytie0x/mcp-loadtest),
an open-source tool that drives a server through the protocol. Its
`lost-update` check gives each write a unique marker, sends the writes
concurrently, and then reads the store once to check that every acknowledged
write is there. Each result below was also checked by counting the store
directly, outside the tool. Full setup, commands and every run, including
clean passes, are in
[field-results.md](https://github.com/clytie0x/mcp-loadtest/blob/main/docs/field-results.md).

| Server (version) | Write tool | Result | Source |
|---|---|---|---|
| `@modelcontextprotocol/server-filesystem` 2.0.0 | `edit_file` on one file | One process, 2, 5 or 10 parallel edits: every call returned success, 1 edit kept, in all 9 runs. One session sending 5 at a time: 12 of 15 acknowledged edits lost, 18/18 calls ok, every run. Since 1.0.0 the in-place write can also leave the file torn. Upstream `main` has the same code. | [file-backed run](https://github.com/clytie0x/mcp-loadtest/blob/main/docs/field-results.md#run-2026-10-19-lost_update-on-file-backed-servers), [pipelined run](https://github.com/clytie0x/mcp-loadtest/blob/main/docs/field-results.md#run-2026-10-19-pipelined-lost_update-on-one-process) |
| `@kazuph/mcp-taskmanager` 1.1.1 | `request_planning` | 8 processes × 5 writes: 39, 37 and 34 of 40 lost, earlier data lost every run. One process: 1 of N kept. | [file-backed run](https://github.com/clytie0x/mcp-loadtest/blob/main/docs/field-results.md#run-2026-10-19-lost_update-on-file-backed-servers) |
| mcp-shrimp-task-manager 1.0.21 | `split_tasks` (append) | One session, 5 at a time: 12 of 15 lost every run. | [more servers](https://github.com/clytie0x/mcp-loadtest/blob/main/docs/field-results.md#run-2026-10-19-pipelined-lost_update-on-more-servers) |
| basic-memory 0.23.2 | `edit_note` (append to one note) | One session, 5 at a time: 6, 9 and 7 of 15 lost, 18/18 calls ok. Fixed on upstream `main` by refusing the losing call (see below); not yet released. | [more servers](https://github.com/clytie0x/mcp-loadtest/blob/main/docs/field-results.md#run-2026-10-19-pipelined-lost_update-on-more-servers) |
| `@danielmarbach/mnemonic-mcp` 1.0.0 | `relate` on one note | N parallel calls (N = 2, 5, 10, 10): N/N succeeded, 1 relation kept every run. Sent one at a time, 10 of 10 kept. | [mnemonic run](https://github.com/clytie0x/mcp-loadtest/blob/main/docs/field-results.md#run-2026-10-19-mnemonic-mcp) |
| mcp-knowledge-graph 1.3.2 | `aim_memory_store` | 8 processes × 5 writes: 38 of 40, 34 of 36 and 38 of 40 lost; in one run, 4 calls failed to parse a half-written file. One process with 2, 5 or 10 parallel calls: exactly 1 entity left every time. | [rerun](https://github.com/clytie0x/mcp-loadtest/blob/main/docs/field-results.md#run-2026-10-19-lost_update-rerun-on-mcp-knowledge-graph) |
| `@modelcontextprotocol/server-memory` 2026.8.31 (since fixed) | `create_entities` | 8 processes × 5 writes: 32, 29, 31, 25 and 26 of 40 lost. Fixed in 1.0.0. | [server-memory run](https://github.com/clytie0x/mcp-loadtest/blob/main/docs/field-results.md#run-2026-10-18-lost_update-on-server-memory) |

In every case the cause is the same: each call reads the whole file or
structure, changes it, and writes it back, with no lock and no conflict
check. In several cases one client on one connection is enough to lose
writes; no second process is needed. That is the situation of an agent that
issues parallel tool calls.

The same evidence shows the fixes are ordinary and already in use:

- **Serialize.** `@modelcontextprotocol/server-memory` 1.0.0 added a lock
  around the read-modify-write and writes through a temporary file and a
  rename: 0 lost in every run. The cost was visible: p95 latency with 8
  concurrent writers went from 22 to 267 ms on the unfixed release to
  between 83 ms and 2.1 s. mcp-memory-keeper 0.14.1 and mcp-server-sqlite 2025.4.25 let SQLite
  serialize the writes: 0 lost.
- **Refuse.** obsidian-mcp 2.0.1 rejects a conflicting append with
  `REVISION_CONFLICT: The note changed after it was read`. Every write it
  acknowledged was in the note. basic-memory's fix on `main` does the same:
  "The note was modified concurrently. Reload the latest content and retry."

A related failure is the deadlock class fixed in
[HKUDS/Vibe-Trading PR #85](https://github.com/HKUDS/Vibe-Trading/pull/85).
A server that passed `initialize` and `tools/list` deadlocked on its first
concurrent `tools/call`, because of lazy initialization. That bug did not
lose writes, but it also comes from assuming calls arrive one at a time.

### Why this belongs in the specification

The calls in this evidence are legal: they use distinct request IDs and each
one is answered. The servers are well-formed by every rule the spec states,
and they still lose data. The client cannot tell, because each lost write
was reported as a success. Neither side breaks a rule, yet a write is lost
without any error. A protocol that standardizes tool calling should say
whose job it is to prevent that.

## Specification

### 1. `server/tools`: new section "Concurrent Tool Calls"

Insert after "Stateful Tools":

> ## Concurrent Tool Calls
>
> A client **MAY** send a `tools/call` request while earlier requests are
> still outstanding, including several calls to the same tool. Separately,
> several clients or server processes may operate on the same underlying
> state. Servers **SHOULD** assume that any two tool calls can execute
> concurrently, and that their responses can be sent in any order.
>
> When a tool call modifies state that another tool call can also modify,
> the server **MUST NOT** return a successful result for a modification whose
> effect is then lost to a concurrent call, unless the tool's documented
> semantics say a later call replaces it. In other words, the outcome of
> concurrent successful calls **MUST** match some order of running them one
> at a time: two `add_item` calls that both succeed leave both items, while
> two `set_status` calls may leave either value, because replacing the value
> is what `set_status` does. To meet this, a server **SHOULD** either:
>
> - serialize conflicting modifications, for example with a lock, a
>   transaction, or a single writer, so that each acknowledged modification
>   is applied; or
> - detect the conflict and return a
>   [tool execution error](#error-handling) (`isError: true`) whose message
>   tells the model the state changed and the call can be retried after
>   reading it again.
>
> This applies equally to calls on one connection and to calls arriving
> through different connections or server processes that share a store.
>
> Clients **SHOULD NOT** assume that concurrent tool calls take effect in the
> order they were sent. A client that needs one call to observe the effect
> of another **SHOULD** wait for the first call's response before sending the
> second.

### 2. `server/tools`, Stateful Tools: one added bullet (non-normative)

Add to the list "When designing handles, servers should consider":

> - **Concurrent use.** Several calls can name the same handle at the same
>   time, for example two `add_item` calls on one basket. See
>   [Concurrent Tool Calls](#concurrent-tool-calls).

### 3. `ToolAnnotations.idempotentHint`: clarify the doc comment (non-normative)

Append to the existing comment in `schema.ts`:

> This hint describes repeated calls with the same arguments. It does not
> indicate whether concurrent calls with different arguments are safe; see
> Concurrent Tool Calls.

### 4. Changelog entry

> Servers **SHOULD** assume tool calls may execute concurrently and **MUST
> NOT** acknowledge a modification that a concurrent call can silently
> overwrite ([SEP-{NUMBER}](...)).

No schema types, messages, capabilities, error codes or annotations are added
or changed.

## Rationale

**Why a MUST NOT on silent loss, and not only a SHOULD.** The requirement is
about what the server reports, not how it stores data. A successful result
that is later silently undone is wrong information sent to the client, and
the client cannot detect it. Silent loss is the one outcome the requirement
forbids. Loss here means an effect that no sequential order of the
successful calls would lose; a tool whose documented job is to replace a
value (last writer wins) does not violate it when a later call replaces
that value. Losing data with an error, refusing the call, or serializing are
all allowed. It does not require transactions, exactly-once execution or
any particular storage. A server whose tools never modify shared state (most
read-only and stateless servers) is not affected.

**Why "serialize or refuse" is a SHOULD with two options.** Both are in use
in published servers (see Motivation), and the right one depends on the
store. A lock is simple for a local file but costs latency under
contention. A conflict error suits stores with versions or revisions and
lets the model decide. A SHOULD leaves room for other correct designs, such
as append-only logs or CRDTs.

**Why the server and not the client.** The client cannot see what state two
tools share, and several clients can share one store. A client that runs
calls one at a time does not protect a store that other clients and
processes also use. Only the server knows its state, so the obligation sits
there. The client sentence only tells clients not to expect an order the
protocol does not give.

**Why put this in `server/tools` and not the base protocol.** Tool calls are
where models issue parallel requests and where servers modify state. Resource
writes are being specified by the Filesystems WG with their own optimistic
concurrency control. This SEP is meant to be consistent with that work: a
conflict error from a tool is the tool-call version of a failed precondition
on a resource write.

### Alternatives considered

- **A new tool annotation**, for example `concurrencySafeHint: false` meaning
  "do not run this tool concurrently with other calls to this server".
  Rejected as the main fix. Annotations are untrusted hints, and a client may
  ignore them. One client honouring the hint cannot serialize against other
  clients or processes on the same store. It also moves a server bug onto
  every client. It may still help clients plan parallel calls; see Open
  Questions.
- **Reusing `idempotentHint`.** Idempotence is about repeats with the same
  arguments. `edit_file` with distinct edits, or appends with distinct
  markers, are not idempotent and should still not be lost. An idempotent
  tool can also lose concurrent writes with different arguments. Changing
  the hint's meaning would also break existing servers' declarations.
- **Saying that servers process requests one at a time.** Rejected. Both
  official SDKs already process them concurrently, it would make slow tools
  block all others, and it would not help across processes.
- **Idempotency keys or request deduplication** (SEP-3182, #3394). These
  address retries producing **duplicate** effects. This SEP addresses
  concurrency producing **lost** effects. They complement each other and
  can share the same section.
- **Documentation only.** See Appendix A. It reaches server authors sooner,
  but it does not define correct behaviour, and implementations judged
  against the spec would still pass.

## Backward Compatibility

No wire-format change. Existing clients and servers interoperate unchanged.

Servers that lose acknowledged writes under concurrent calls are not
conformant under this SEP. They are already incorrect from a user's point of
view, and the fix is local to the server (a lock, a transaction, or a
conflict error). A server that switches to conflict errors will return
`isError: true` results it did not return before. Models already handle tool
execution errors, and clients need no change.

## Security Implications

Concurrent modification without serialization can also be a security
problem. A permission or audit record written by a read-modify-write can be
silently dropped the same way. Serializing writes adds lock contention, which
an attacker could use to slow a server down. The existing requirement to
"rate limit tool invocations" applies. No new attack surface is added.

## Reference Implementation

There is no protocol change to implement. The prototype requirement is met
by:

- Servers that already meet the requirements:
  `@modelcontextprotocol/server-memory` 1.0.0 and later (lock and atomic
  rename), obsidian-mcp 2.0.1 (`REVISION_CONFLICT` refusal), basic-memory
  `main` (conflict error).
- A runnable check: `mcp-loadtest lost-update`
  ([docs](https://github.com/clytie0x/mcp-loadtest/blob/main/docs/examples/lost-update.md)),
  which fails a server that loses an acknowledged write and passes one that
  serializes or refuses (refused writes are counted separately). It has
  pinned regression tests against two affected releases.

### Testing Plan

The MUST NOT changes observable behaviour, so the SEP needs a conformance
scenario and a traceability mapping before it can reach `Final`. A
conformance scenario can only be generic if the harness knows which tools
write and how to read the result back, so the plan is a test-server fixture
with an append-style write tool and a read tool. The scenario sends N
`tools/call` writes, each with a unique marker, without waiting for
responses, then reads once, and checks that every write that returned a
successful result is visible; writes that returned `isError` are allowed.
It maps to the new MUST NOT. The author offers to contribute the fixture and
scenario, based on mcp-loadtest's `lost_update` check, which already does
this against real servers.

## Open Questions

1. **MUST NOT or SHOULD NOT.** Is a MUST NOT on silent loss acceptable, given
   that it makes some existing servers non-conformant? The fallback is the
   same text at SHOULD NOT, as an Informational SEP.
2. **A planning hint for clients.** Is there value, for the Tool Annotations
   IG, in a hint that lets a client avoid running a tool in parallel,
   understood as an optimization and never a substitute for the server-side
   requirement?
3. **Base protocol wording.** Should `basic/index` also say that a receiver
   may process outstanding requests concurrently and answer them in any
   order, for all methods and not only tools?
4. **A standard conflict error.** Should a conflict refusal carry a
   well-known marker (for example in `_meta`) so clients can retry
   automatically, or is a plain-text `isError` result enough? This overlaps
   with #3394's retry work.

## Acknowledgments

{to fill in}

---

## Appendix A: companion docs text (no SEP needed)

For a docs-only PR to the server guide at
`docs/docs/<revision>/develop/build-server.mdx` in the specification repository,
placed after the existing "Best Practices" logging section (once, outside the
per-language tabs). The same two paragraphs, with the code example in one
language, fit the TypeScript and Python SDK tool pages.

> ### Concurrent tool calls
>
> Your tool handlers can run at the same time. A model often issues several
> tool calls at once, and the SDK starts each handler without waiting for
> the others. Several clients, or several copies of your server, can also
> share one file or database. If two calls read the same data, change it and
> write it back, the second write can erase the first while both calls
> report success.
>
> If a tool modifies data that another call can also modify, protect it.
> Either hold a lock (or use a database transaction) around the whole
> read-modify-write, or detect that the data changed since it was read and
> return a tool error (`isError: true`) that tells the model to read it again
> and retry. A lock inside one process does not protect a file that other
> processes write: use a lock file or a database transaction around the whole
> read-modify-write, or a compare-and-swap check. Writing to a temporary file
> and renaming it only stops readers from seeing a half-written file; on its
> own it does not stop two processes from overwriting each other's changes.
>
> ```python
> import asyncio
>
> _store_lock = asyncio.Lock()
>
> @mcp.tool()  # mcp = MCPServer("notes"), as in the guide above
> async def add_note(text: str) -> str:
>     async with _store_lock:  # one read-modify-write at a time in this process
>         notes = await load_notes()
>         notes.append(text)
>         await save_notes(notes)
>     return "saved"
> ```

The code is illustrative. `load_notes` and `save_notes` stand for the
server's own storage functions, and the decorator should match the guide's
current Python example when the PR is written.
