#!/usr/bin/env bash
# Review 111 N-1 (and reviews 123/127/128: F-1/S-1/S-2, R-1): the real corpus and the previous release's
# published binary are fetched or built once, into the project's own git-ignored fixtures/ folder — never
# into ~/Desktop/prikk/corpus-scratch/, a /tmp worktree, or a local rebuild. This is the one shared
# script that does it; no task builds its own cache.
#
# Usage: tools/fixtures.sh <fetch|verify>
#   fetch:  fetches whatever is missing under fixtures/ (network: gh, and whatever
#           tools/corpus/real.toml's entries need — git, hg, rsync or svnrdump), then checks
#           everything, present or just fetched, with no further network.
#   verify: checks everything against its pin/hash/checksum, with **no fetch and no network at all**
#           (review 127 F-1). An absent entry is a **failure** (review 128 R-1): a step that needs a
#           fixture must not pass without it — run `fetch` first. Creates no directory (review 127 S-2):
#           only `fetch` does.
#
# Review 128 R-1: every section below runs regardless of an earlier one's result — a failing fetch must
# not skip the checks that would otherwise have caught it, and a failing check of one fixture (the
# corpus) must not leave the other (the binary) unchecked. The exit code is 1 if anything failed, 0 only
# if every section passed.
#
# fixtures/corpus/<kind>/<name>        the real corpus cache (tools/corpus/real.toml's entries),
#                                      fetched by `brygge-bench real-fetch` and checked by
#                                      `brygge-bench real-check` (never both at once: review 127 F-1
#                                      split them, since the combined command used to fetch even in
#                                      "verify" mode, and re-synced CVS entries from the network on
#                                      every run).
# fixtures/previous-release/<version>/brygge
#                                      the previous release's published x86_64 Linux binary, fetched with
#                                      `gh release download`, verified against its own .sha256 and
#                                      `gh attestation verify` at fetch time, and against the checksum
#                                      recorded below at every later check. Never a local rebuild.
# Full strict mode is safe here: every command whose failure must not abort the whole script (the
# fetch/check steps, review 128 R-1) is already the condition of an `if`/`||`, which `set -e` exempts.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
root=$PWD
fixtures="$root/fixtures"
checksum_file="$root/tools/fixtures.sha256"

usage() {
    echo "usage: tools/fixtures.sh <fetch|verify>" >&2
    exit 2
}

[ $# -eq 1 ] || usage
case "$1" in
    fetch | verify) mode=$1 ;;
    *) usage ;;
esac

# Review 127 S-1: one source for the previous version, so a new release changes one file (this one) and
# the two can never disagree. The path is `fixtures/previous-release/<version>/brygge`; refuse rather
# than guess if the file names more than one. This is setup, not a per-fixture check, so it still fails
# fast: nothing below knows which binary to work with until this resolves.
mapfile -t prev_versions < <(
    grep -oE 'fixtures/previous-release/[^/]+/brygge$' "$checksum_file" | sed -E 's#.*/([^/]+)/brygge$#\1#' | sort -u
)
case "${#prev_versions[@]}" in
    0)
        echo "fixtures: $checksum_file names no fixtures/previous-release/<version>/brygge entry" >&2
        exit 1
        ;;
    1) prev_version=${prev_versions[0]} ;;
    *)
        echo "fixtures: $checksum_file names more than one previous release: ${prev_versions[*]}" >&2
        exit 1
        ;;
esac
prev_asset="brygge-x86_64-unknown-linux-gnu.tar.gz"
prev_dir="$fixtures/previous-release/$prev_version"
prev_bin="$prev_dir/brygge"
prev_rel="fixtures/previous-release/$prev_version/brygge"

fetch_prev_release() {
    if [ -f "$prev_bin" ]; then
        echo "fixtures: $prev_bin already present"
        return 0
    fi
    echo "fixtures: fetching brygge $prev_version's published Linux binary ..."
    work=$(mktemp -d)
    if gh release download "$prev_version" --repo prikk-vcs/brygge \
        --pattern "$prev_asset" --pattern "$prev_asset.sha256" --dir "$work" &&
        (cd "$work" && sha256sum -c "$prev_asset.sha256") &&
        gh attestation verify "$work/$prev_asset" --repo prikk-vcs/brygge &&
        tar -xzf "$work/$prev_asset" -C "$prev_dir" brygge; then
        chmod +x "$prev_bin"
        rm -rf "$work"
        echo "fixtures: fetched and attestation-verified $prev_bin"
        return 0
    fi
    rm -rf "$work"
    echo "fixtures: failed to fetch brygge $prev_version's published binary" >&2
    return 1
}

check_prev_release() {
    if [ ! -f "$prev_bin" ]; then
        echo "fixtures: $prev_bin is missing" >&2
        return 1
    fi
    if ! grep -qF "  $prev_rel" "$checksum_file"; then
        echo "fixtures: no recorded checksum for $prev_rel in $checksum_file" >&2
        return 1
    fi
    if ! (cd "$root" && sha256sum -c <(grep -F "  $prev_rel" "$checksum_file")); then
        return 1
    fi
    echo "fixtures: $prev_bin matches the recorded checksum"
    return 0
}

failed=0

if [ "$mode" = fetch ]; then
    mkdir -p "$fixtures/corpus" "$prev_dir"

    echo "::group::the real corpus: fetch (fixtures/corpus)"
    (cd tools/bench && cargo run --release -q -- real-fetch "$root/tools/corpus/real.toml" "$fixtures/corpus") ||
        failed=1
    echo "::endgroup::"

    echo "::group::the previous release's published binary ($prev_version): fetch"
    fetch_prev_release || failed=1
    echo "::endgroup::"
fi

echo "::group::the real corpus: check (fixtures/corpus, no network)"
(cd tools/bench && cargo run --release -q -- real-check "$root/tools/corpus/real.toml" "$fixtures/corpus") ||
    failed=1
echo "::endgroup::"

echo "::group::the previous release's published binary ($prev_version): check (no network)"
check_prev_release || failed=1
echo "::endgroup::"

exit "$failed"
