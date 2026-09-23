#!/usr/bin/env bash
# Is <crate> at <X.Y.Z> in the crates.io sparse index? (RFC 012 D-5: publication skips what is done.)
#
# Exit status: 0 the version is in the index (a yanked version counts: it can never be published again),
# 1 it is not, 2 the answer could not be established (usage error, network failure, unexpected response).
# Never guesses: an unexpected HTTP status is 2, not "not published".
#
# Usage: tools/crate-published.sh <crate> <X.Y.Z>
set -euo pipefail

if [ $# -ne 2 ]; then
    echo "usage: tools/crate-published.sh <crate> <X.Y.Z>" >&2
    exit 2
fi
crate=$1
version=$2
if ! [[ $crate =~ ^[a-z0-9_-]+$ ]]; then
    echo "crate-published: '$crate' is not a crate name" >&2
    exit 2
fi
if ! [[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "crate-published: '$version' is not a version of the form X.Y.Z" >&2
    exit 2
fi

# The sparse-index path of a crate name (https://doc.rust-lang.org/cargo/reference/registry-index.html).
case ${#crate} in
    1) path="1/$crate" ;;
    2) path="2/$crate" ;;
    3) path="3/${crate:0:1}/$crate" ;;
    *) path="${crate:0:2}/${crate:2:2}/$crate" ;;
esac

# BRYGGE_TEST_CRATES_INDEX points the tool tests at a local server; nothing else sets it.
index=${BRYGGE_TEST_CRATES_INDEX:-https://index.crates.io}
body=$(mktemp)
trap 'rm -f "$body"' EXIT

status=$(curl --silent --show-error --location --retry 3 --retry-all-errors \
    --user-agent "brygge-release-tools (https://github.com/prikk-vcs/brygge)" \
    --output "$body" --write-out '%{http_code}' "$index/$path") || {
    echo "crate-published: could not reach $index/$path" >&2
    exit 2
}

case $status in
    200)
        if grep -qF "\"vers\":\"$version\"" "$body"; then
            exit 0
        fi
        exit 1
        ;;
    404)
        # The crate itself is not in the index, so no version of it is.
        exit 1
        ;;
    *)
        echo "crate-published: $index/$path answered HTTP $status" >&2
        exit 2
        ;;
esac
