#!/usr/bin/env bash
# Fail if a GitHub release already exists for <X.Y.Z> (RFC 012 D-5/D-6: a dispatch for a tag that is already
# released never edits the release). Exit status: 0 no release exists, 1 one does, 2 the answer could not
# be established (usage error, or `gh` failed for another reason). Never guesses.
#
# Uses the `gh` CLI, authenticated by GH_TOKEN (the job's GITHUB_TOKEN); GH_REPO or GITHUB_REPOSITORY
# names the repository, as `gh` reads them.
#
# Usage: tools/check-release-absent.sh <X.Y.Z>
set -euo pipefail

if [ $# -ne 1 ] || ! [[ $1 =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "usage: tools/check-release-absent.sh <X.Y.Z>" >&2
    exit 2
fi
tag=$1

if out=$(gh release view "$tag" 2>&1); then
    echo "FAILED: a GitHub release for $tag already exists; this workflow never edits an existing release" >&2
    exit 1
fi
case $out in
    *"release not found"*)
        echo "no GitHub release exists for $tag yet"
        ;;
    *)
        echo "check-release-absent: could not tell whether a release exists for $tag: $out" >&2
        exit 2
        ;;
esac
