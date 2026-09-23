#!/usr/bin/env bash
# The release preconditions that concern the tag itself (RFC 012 D-1). Fails, naming the reason, unless
#   - the tag is annotated;
#   - it points at a commit that is reachable from origin/main;
#   - the checked-out tree is that commit (so what is verified is what is tagged);
#   - every workspace crate's version equals the tag;
#   - CHANGELOG.md has exactly one `## [X.Y.Z]` heading for it, with text (tools/release-notes.sh).
# The release workflow runs it first, in the `verify` job; a person can run it before pushing a tag.
#
# It checks the repository it is run in (the current directory's), wherever the script itself lives.
#
# Usage: tools/check-release-tag.sh <X.Y.Z>       (the tag is the bare version)
# Needs: git with the tag and origin/main fetched, cargo, jq.
set -euo pipefail

if [ $# -ne 1 ] || ! [[ $1 =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "usage: tools/check-release-tag.sh <X.Y.Z>" >&2
    exit 2
fi
tag=$1

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
cd "$(git rev-parse --show-toplevel)" || {
    echo "check-release-tag: run it inside the brygge repository" >&2
    exit 2
}

fail() {
    echo "FAILED: $*" >&2
    exit 1
}

# The tag exists and is an annotated tag object (a lightweight tag is a bare ref to a commit).
kind=$(git cat-file -t "refs/tags/$tag" 2>/dev/null) || fail "the tag '$tag' does not exist in this repository"
[ "$kind" = tag ] || fail "the tag '$tag' is not annotated (it is a $kind); create it with 'git tag -a'"
commit=$(git rev-parse --verify "refs/tags/$tag^{commit}")

# It is on main: the tagged commit is an ancestor of (or is) origin/main.
git rev-parse --verify --quiet refs/remotes/origin/main >/dev/null || fail "origin/main is not fetched, so the tag cannot be checked against it"
git merge-base --is-ancestor "$commit" refs/remotes/origin/main || fail "the tagged commit $commit is not reachable from origin/main"

# What is checked out is what is tagged.
head=$(git rev-parse --verify HEAD)
[ "$head" = "$commit" ] || fail "the checked-out commit $head is not the tagged commit $commit"

# Every workspace crate carries the tag's version.
versions=$(cargo metadata --no-deps --format-version 1 --offline | jq -r '.packages[] | "\(.name) \(.version)"')
[ -n "$versions" ] || fail "cargo metadata found no workspace crate"
bad=$(printf '%s\n' "$versions" | awk -v v="$tag" '$2 != v { print "  " $1 " is " $2 }')
[ -z "$bad" ] || fail "the tag is $tag but these crates have another version:
$bad"

# The changelog has exactly one heading for it, with text.
"$here/release-notes.sh" "$tag" >/dev/null || fail "CHANGELOG.md is not ready for $tag (see the message above)"

echo "OK: $tag is an annotated tag on origin/main at $commit; all crates are $tag; CHANGELOG.md has its section"
