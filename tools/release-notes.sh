#!/usr/bin/env bash
# Print the release notes for one version: the `## [X.Y.Z]` section of CHANGELOG.md (RFC 012 D-6).
#
# The notes run from after the heading up to, and not including, the line that begins "The detailed
# changes" (the changelog's own divider between the release notes and the per-commit record). If a section
# has no such line, they run up to the next `## [` heading. Blank lines at either end are dropped.
#
# Fails (non-zero, nothing on stdout) when the version is not `X.Y.Z`, when the section is missing or
# appears more than once, or when it has no text. The release workflow relies on that: a release needs
# exactly one heading for its tag.
#
# It reads the CHANGELOG.md of the repository it is run in (the current directory's), wherever the script
# itself lives, so the release workflow can run its own copy on the tagged tree.
#
# Usage: tools/release-notes.sh <X.Y.Z>
set -euo pipefail

if [ $# -ne 1 ]; then
    echo "usage: tools/release-notes.sh <X.Y.Z>" >&2
    exit 2
fi
version=$1
if ! [[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "release-notes: '$version' is not a version of the form X.Y.Z" >&2
    exit 2
fi

cd "$(git rev-parse --show-toplevel)" || {
    echo "release-notes: run it inside the brygge repository" >&2
    exit 2
}
changelog=CHANGELOG.md
heading="## [$version]"

# How many headings does this version have? Exactly one is required. The heading is followed by a space
# or the end of the line, so that `## [0.1.1]` is never mistaken for another version.
count=$(awk -v h="$heading" 'index($0, h) == 1 { rest = substr($0, length(h) + 1); if (rest == "" || rest ~ /^[ \t]/) n++ } END { print n + 0 }' "$changelog")
if [ "$count" -ne 1 ]; then
    echo "release-notes: $changelog must have exactly one '$heading' heading, found $count" >&2
    exit 1
fi

notes=$(awk -v h="$heading" '
    !inside {
        if (index($0, h) == 1) { inside = 1 }
        next
    }
    /^## \[/ { exit }
    /^The detailed changes/ { exit }
    { print }
' "$changelog" | awk '
    # Drop blank lines at the start, and at the end (a blank line is held until text follows it).
    NF { for (i = 0; i < held; i++) print ""; held = 0; started = 1; print; next }
    started { held++ }
')

if [ -z "$notes" ]; then
    echo "release-notes: the '$heading' section of $changelog has no text" >&2
    exit 1
fi
printf '%s\n' "$notes"
