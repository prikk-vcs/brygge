#!/usr/bin/env bash
# Should the GitHub release for <X.Y.Z> be marked "Latest"? (RFC 012 D-6.) Prints `true` when <X.Y.Z> is higher
# than every release already on GitHub, and `false` otherwise, so that releasing an older tag (0.1.0 after
# 0.1.1) never points users at the older version. The release workflow passes the answer to
# `gh release create --latest=<answer>`; `gh` marks a new release Latest by default, which is the defect this
# tool exists to prevent.
#
# Versions are compared as numbers, component by component (`0.1.10` is higher than `0.1.9`). Release tags that
# are not of the form X.Y.Z are ignored. With no release yet the answer is `true`. Drafts and prereleases
# count: they hold their version.
#
# Exit status: 0 with the answer on stdout; 2 when the answer could not be established (a usage error, `gh`
# failing, or more releases than can be listed). It never guesses: a wrong `true` is the defect itself.
#
# Uses the `gh` CLI, authenticated by GH_TOKEN; GH_REPO or GITHUB_REPOSITORY names the repository.
#
# Usage: tools/release-latest.sh <X.Y.Z>
# Needs: gh, jq, sort -V.
set -euo pipefail

limit=1000

if [ $# -ne 1 ] || ! [[ $1 =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "usage: tools/release-latest.sh <X.Y.Z>" >&2
    exit 2
fi
asked=$1

listing=$(gh release list --limit "$limit" --json tagName 2>&1) || {
    echo "release-latest: could not list the releases: $listing" >&2
    exit 2
}
count=$(printf '%s' "$listing" | jq 'length' 2>/dev/null) || {
    echo "release-latest: gh did not return a JSON list of releases" >&2
    exit 2
}
if [ "$count" -ge "$limit" ]; then
    echo "release-latest: $count releases is as many as can be listed; cannot tell which is the highest" >&2
    exit 2
fi

existing=$(printf '%s' "$listing" | jq -r '.[].tagName' | grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' || true)

# It is the latest only if it is not already released and sorts above every existing version.
if printf '%s\n' "$existing" | grep -qxF "$asked"; then
    echo false
    exit 0
fi
highest=$(printf '%s\n%s\n' "$existing" "$asked" | grep -v '^$' | sort -V | tail -n 1)
if [ "$highest" = "$asked" ]; then
    echo true
else
    echo false
fi
