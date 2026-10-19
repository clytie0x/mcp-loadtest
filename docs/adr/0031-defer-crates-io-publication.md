# 31. Defer crates.io publication (supersedes 0020)

Date: 2026-10-19
Status: Accepted

## Context

[ADR 0020](0020-publish-to-crates-io.md) proposed publishing the crates to
crates.io at the next release. It was never approved or executed, and
`docs/RELEASE.md` has always treated crates.io as a separate, irreversible
maintainer decision. Publishing is append-only: a version can be yanked but
never deleted, and a name, once taken, can never be reclaimed.

Users can already install without a registry. Each release ships checksummed
prebuilt archives for four platforms on an immutable GitHub Release, and
`cargo install --git <repo> --tag <tag> --locked mcp-loadtest-cli` builds from
the exact tag. Nobody outside the project has asked for a registry dependency.

## Decision

Do not publish to crates.io for now, and do not reserve any crate names.
Installs stay on the GitHub Release archives and `cargo install --git
--tag`.

Revisit when an external user asks for a registry dependency. That is the
trigger ADR 0015 set. ADR 0020's proposal is kept unchanged for reference.

## Consequences

- No `cargo add mcp-loadtest`, no docs.rs, and no crates.io discoverability.
- The crate names stay unclaimed and could be taken by someone else. This is
  the risk ADR 0015 accepted; this decision accepts it again.
- `docs/RELEASE.md` stays as it is: crates.io is not part of any release.
