#!/usr/bin/env bash
# Create the annotated release tag vVERSION on SHA after checking that the
# commit is a valid release candidate (ADR 0028).
# Usage: bash scripts/release-tag.sh VERSION SHA [--push]
#
# RELEASE_BASE_REF (default origin/main) is the branch SHA must be on, and
# RELEASE_REMOTE (default origin) is where the tag is checked and pushed.
set -euo pipefail

fail() {
    echo "release-tag: $*" >&2
    exit 1
}

[ "$#" -ge 2 ] && [ "$#" -le 3 ] || fail "usage: release-tag.sh VERSION SHA [--push]"
version="$1"
sha="$2"
push=false
if [ "$#" -eq 3 ]; then
    [ "$3" = "--push" ] || fail "unknown option: $3"
    push=true
fi
base_ref="${RELEASE_BASE_REF:-origin/main}"
remote="${RELEASE_REMOTE:-origin}"

[[ "$version" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] ||
    fail "version must be MAJOR.MINOR.PATCH without a v prefix: $version"
[[ "$sha" =~ ^[0-9a-f]{40}$ ]] || fail "sha must be a full 40-character commit id: $sha"
tag="v${version}"

git cat-file -e "${sha}^{commit}" 2>/dev/null || fail "commit not found: $sha"
git merge-base --is-ancestor "$sha" "$base_ref" ||
    fail "$sha is not on $base_ref; release only reviewed, merged commits"

cargo_version="$(git show "${sha}:Cargo.toml" | awk -F'"' '/^version = "/ { print $2; exit }')"
[ "$cargo_version" = "$version" ] ||
    fail "Cargo.toml at $sha has version ${cargo_version:-<none>}, not $version"
git show "${sha}:CHANGELOG.md" | grep -F "## [${version}]" >/dev/null ||
    fail "CHANGELOG.md at $sha has no ## [${version}] section"

[ -z "$(git tag --list "$tag")" ] || fail "tag $tag already exists locally"
remote_tag="$(git ls-remote --tags "$remote" "refs/tags/${tag}")" ||
    fail "cannot list tags on $remote"
[ -z "$remote_tag" ] || fail "tag $tag already exists on $remote; release tags are immutable"

git tag -a "$tag" -m "mcp-loadtest ${tag}" "$sha"
[ "$(git cat-file -t "$tag")" = "tag" ] || fail "$tag is not an annotated tag"
[ "$(git rev-parse "${tag}^{commit}")" = "$sha" ] || fail "$tag does not point at $sha"

if [ "$push" = true ]; then
    git push "$remote" "refs/tags/${tag}"
fi
echo "release-tag: $tag -> $sha"
