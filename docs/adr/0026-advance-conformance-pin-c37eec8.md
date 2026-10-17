# 26. Advance the reviewed conformance pin to c37eec8

Date: 2026-10-17
Status: Accepted (updates the conformance pin recorded in [0023](0023-mcp-2026-final-reconciliation.md))

## Context

`run-official-conformance.sh` and `.ps1` fail when the reviewed pin is no
longer upstream `main`, so a pin moves only after review. Upstream
`modelcontextprotocol/conformance` moved from `49103de` to `c37eec8`
(0.2.0-alpha.12, 30 commits), and the check turned CI red on `main`.

Review of the 30 commits, as they affect our client leg:

- `232a901` adds `requirements/2026-07-28.yaml`, the frozen list of client
  scenarios that conformance to `2026-07-28` requires. It names exactly the
  32 scenarios our scope manifest already executed.
- `98535fb` adds `resource-parameter-matches-prm`: the RFC 8707 `resource`
  parameter must equal the protected-resource metadata value byte for byte.
  Our client failed it in `auth/metadata-var2`: it re-serialized the
  identifier through `url::Url`, which appends `/` to a pathless origin.
- `81eb1c3` adds the client scenario `json-schema-2020-12-preservation`,
  which the requirements file lists as `not_scored` (added after release).
- The remaining commits change server-side, authorization-server, skills,
  DPoP, and tier-check code, none of which our client leg runs.

The vendored specification source (`71e3069`) is unchanged.

## Decision

- Fix the client to send `AuthorizationContext::resource_identifier`, the
  exact published string, in authorization, token, refresh, and
  client-credentials requests and token bindings.
- Execute `json-schema-2020-12-preservation`, with the adapter round-tripping
  the advertised `inputSchema` verbatim, instead of listing it as not
  executed. The PowerShell runner requires every inventory scenario to be
  executed, and the client passes it.
- Move `conformance_ref` to `c37eec888e1c6ff140af79987a40008548b7cc5f` in
  both runners.

## Consequences

- A local run of `run-official-conformance.sh` at the new pin passes all
  33 executed client scenarios.
- Configurations whose protected resource publishes a pathless identifier now
  receive tokens bound to that exact identifier. Before, an authorization
  server that compares exactly would have rejected the request or issued a
  token for `…/`.
- The pin will fall behind upstream `main` again, and the next move needs
  the same review.
