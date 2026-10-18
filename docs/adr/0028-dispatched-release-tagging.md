# 28. Create release tags from a dispatched workflow

Date: 2026-10-18
Status: Accepted (adds a second path to Gate C of `docs/RELEASE.md`)

## Context

A release starts when the annotated tag `vX.Y.Z` reaches GitHub, which
triggers `release.yml`. Until now only a maintainer pushing from a local
clone could do that. The maintainer works with a coding agent that prepares
releases end to end (version bump, changelog, Gate A evidence), but the
agent's session can push only its own branch. It cannot push tags, so every
release stopped at a hand-off to the maintainer's machine.

The maintainer asked for the agent to be able to complete the release
itself, after their confirmation.

## Decision

Add `.github/workflows/tag-release.yml`, dispatched with `version` and a
full `sha`:

- **Runs only from `main`.** A dispatch from any other ref is skipped, so
  only the reviewed workflow and script can create a tag.
- **Requires green CI on the commit.** At least one successful `ci.yml`
  run must exist for `sha`.
- **Validates in `scripts/release-tag.sh`.** The version must be
  `MAJOR.MINOR.PATCH`, the commit must be on `main`, `Cargo.toml` and
  `CHANGELOG.md` at that commit must name the version, and the tag must not
  exist locally or on the remote. The script then creates and pushes the
  annotated tag. `scripts/test-release-tag.sh` covers each refusal against
  throwaway repositories and runs in CI.
- **Dispatches `release.yml`.** Tags pushed with `GITHUB_TOKEN` do not
  trigger push workflows, so the job starts the release with
  `workflow_dispatch` and `tag=vX.Y.Z`, the existing rerun path.
  `release.yml` is unchanged and still enforces its own preflight: an
  annotated tag matching the workspace version, the changelog section,
  immutable releases, and the full engineering gate.

The job holds `contents: write` (to push the tag) and `actions: write` (to
dispatch `release.yml`). Everything else stays read-only.

## Alternatives considered

- **Keep the manual tag push only.** This leaves every release blocked on
  the maintainer's local clone, which is the problem being solved.
- **Tag on merge to `main` when the version changes.** This removes the
  explicit release decision. Merging and releasing are separate
  confirmations in `docs/RELEASE.md`, and an immutable release must stay a
  deliberate act.
- **A personal access token so tag pushes trigger `release.yml`.** This
  adds a long-lived credential with write access. Dispatching with
  `GITHUB_TOKEN` achieves the same result without one.

## Consequences

- Dispatching `tag-release` is the release action and needs the same
  explicit maintainer confirmation as a manual tag push. Anyone with write
  access can dispatch it, which matches who can push tags today.
- The commit must be on `main` before it can be tagged, so a release now
  follows the merge. The manual path in Gate C still allows tagging before
  the merge, as v0.2.0 was.
- The tagger shown on the tag is `github-actions[bot]`, not the maintainer.
  The dispatching user is recorded in the workflow run.
