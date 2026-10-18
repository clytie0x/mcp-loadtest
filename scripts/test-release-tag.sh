#!/usr/bin/env bash
# Contract tests for scripts/release-tag.sh against throwaway repositories.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
script="${repo_root}/scripts/release-tag.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

fail() {
    echo "release tag test failed: $*" >&2
    exit 1
}

expect_refusal() {
    local context="$1" needle="$2"
    shift 2
    local out
    if out="$(bash "$script" "$@" 2>&1)"; then
        fail "${context}: expected a refusal, got success: ${out}"
    fi
    case "$out" in
        *"$needle"*) ;;
        *) fail "${context}: expected <${needle}> in: ${out}" ;;
    esac
}

commit_release() {
    local version="$1" changelog="$2" message="$3"
    printf '[workspace.package]\nversion = "%s"\n' "$version" >Cargo.toml
    printf '# Changelog\n\n%s\n' "$changelog" >CHANGELOG.md
    git add Cargo.toml CHANGELOG.md
    git commit -q -m "$message"
    git rev-parse HEAD
}

git init -q --bare "$work/remote.git"
git init -q -b main "$work/repo"
cd "$work/repo"
git config user.name "release-tag test"
git config user.email "release-tag@example.invalid"
git remote add origin "$work/remote.git"

good="$(commit_release 1.2.3 '## [1.2.3] — Release contents' 'release 1.2.3')"
mismatch="$(commit_release 1.2.4 '## [1.2.3] — Release contents' 'bump without changelog')"
git push -q origin main
git fetch -q origin
git checkout -q -b feature
unmerged="$(commit_release 1.2.5 '## [1.2.5] — Release contents' 'unmerged release')"
git checkout -q main

expect_refusal "v prefix" "MAJOR.MINOR.PATCH" v1.2.3 "$good"
expect_refusal "two-part version" "MAJOR.MINOR.PATCH" 1.2 "$good"
expect_refusal "short sha" "full 40-character" 1.2.3 "${good:0:12}"
expect_refusal "unknown option" "unknown option" 1.2.3 "$good" --force
expect_refusal "missing commit" "commit not found" 1.2.3 "$(printf '0%.0s' {1..40})"
expect_refusal "commit off main" "is not on origin/main" 1.2.5 "$unmerged"
expect_refusal "version mismatch" "has version 1.2.4, not 1.2.3" 1.2.3 "$mismatch"
expect_refusal "missing changelog section" "no ## [1.2.4] section" 1.2.4 "$mismatch"
[ -z "$(git tag --list)" ] || fail "a refused run left a tag behind"

out="$(bash "$script" 1.2.3 "$good" --push 2>&1)" || fail "valid release refused: ${out}"
[ "$(git cat-file -t v1.2.3)" = "tag" ] || fail "v1.2.3 is not annotated"
[ "$(git rev-parse 'v1.2.3^{commit}')" = "$good" ] || fail "v1.2.3 points at the wrong commit"
[ -n "$(git ls-remote --tags origin refs/tags/v1.2.3)" ] || fail "v1.2.3 was not pushed"

expect_refusal "local tag exists" "already exists locally" 1.2.3 "$good"
git tag -d v1.2.3 >/dev/null
expect_refusal "remote tag exists" "already exists on origin" 1.2.3 "$good"

echo "release tag contract: all tests passed"
