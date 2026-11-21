#!/usr/bin/env bash
# Review 111 N-1: the real corpus and the previous release's published binary are fetched or built
# once, into the project's own git-ignored fixtures/ folder — never into ~/Desktop/prikk/corpus-scratch/,
# a /tmp worktree, or a local rebuild. This is the one shared script that does it; no task builds its
# own cache.
#
# Usage: tools/fixtures.sh <fetch|verify>
#   fetch:  fetches or builds whatever is missing under fixtures/. Already-present, already-verified
#           entries are left alone. Needs network (gh, and whatever tools/corpus/real.toml's entries need:
#           git, hg, or rsync/svnadmin for the dump-based ones) and a release build of brygge-bench.
#   verify: verifies everything present against its pin/hash/checksum, with no fetch. Exits 1, naming the
#           first mismatch, if anything present does not match. An entry that is simply absent is reported,
#           not treated as a failure — run `fetch` first.
#
# fixtures/corpus/<kind>/<name>        the real corpus cache (tools/corpus/real.toml's entries).
# fixtures/previous-release/<version>/brygge
#                                      the previous release's published x86_64 Linux binary, fetched with
#                                      `gh release download`, verified against its own .sha256 and
#                                      `gh attestation verify` at fetch time, and against the checksum
#                                      recorded below at every later `verify`. Never a local rebuild.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
root=$PWD
fixtures="$root/fixtures"

# The previous release this cycle's N-2 cross-version check runs against. Update this, and
# tools/fixtures.sha256's entry, when a newer release becomes "the previous one".
prev_version=0.7.1
prev_asset="brygge-x86_64-unknown-linux-gnu.tar.gz"
prev_dir="$fixtures/previous-release/$prev_version"
prev_bin="$prev_dir/brygge"
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

mkdir -p "$fixtures/corpus" "$prev_dir"

echo "::group::the real corpus (fixtures/corpus)"
if [ "$mode" = fetch ]; then
    # brygge-bench's own fetch_and_verify already checks every entry's pin/hash whether or not the cache
    # was already present, and refuses a mismatch rather than silently reusing stale data — so `fetch` and
    # `verify` both just run it; `fetch` is the only mode that needs network when the cache is empty.
    (cd tools/bench && cargo run --release -q -- real "$root/tools/corpus/real.toml" "$fixtures/corpus")
elif [ -d "$fixtures/corpus" ] && [ -n "$(find "$fixtures/corpus" -mindepth 1 -print -quit)" ]; then
    (cd tools/bench && cargo run --release -q -- real "$root/tools/corpus/real.toml" "$fixtures/corpus")
else
    echo "fixtures: fixtures/corpus is empty; nothing to verify yet (run 'tools/fixtures.sh fetch' first)"
fi
echo "::endgroup::"

echo "::group::the previous release's published binary ($prev_version)"
if [ "$mode" = fetch ] && [ ! -f "$prev_bin" ]; then
    echo "fixtures: fetching brygge $prev_version's published Linux binary ..."
    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT
    gh release download "$prev_version" --repo prikk-vcs/brygge \
        --pattern "$prev_asset" --pattern "$prev_asset.sha256" --dir "$work"
    (cd "$work" && sha256sum -c "$prev_asset.sha256")
    gh attestation verify "$work/$prev_asset" --repo prikk-vcs/brygge
    tar -xzf "$work/$prev_asset" -C "$prev_dir" brygge
    chmod +x "$prev_bin"
    rm -rf "$work"
    trap - EXIT
    echo "fixtures: fetched and attestation-verified $prev_bin"
fi

if [ -f "$prev_bin" ]; then
    rel="fixtures/previous-release/$prev_version/brygge"
    if ! grep -qF "  $rel" "$checksum_file"; then
        echo "fixtures: no recorded checksum for $rel in $checksum_file" >&2
        exit 1
    fi
    (cd "$root" && sha256sum -c <(grep -F "  $rel" "$checksum_file"))
    echo "fixtures: $prev_bin matches the recorded checksum"
elif [ "$mode" = verify ]; then
    echo "fixtures: $prev_bin is missing; nothing to verify yet (run 'tools/fixtures.sh fetch' first)"
fi
echo "::endgroup::"
