#!/usr/bin/env bash
# Smoke-test an installed `brygge` (RFC 012 D-6): it reports the expected version and an IR contract
# version, and it can decode, verify and verify against the source a small generated Git repository, each
# with exit status 0 and a `pass` result. The release workflow runs it on the binary that
# `cargo install --locked brygge --version X.Y.Z` built from crates.io.
#
# Usage: tools/smoke-test.sh <X.Y.Z> <path-to-brygge>
# Needs: git.
set -euo pipefail

if [ $# -ne 2 ] || ! [[ $1 =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "usage: tools/smoke-test.sh <X.Y.Z> <path-to-brygge>" >&2
    exit 2
fi
version=$1
brygge=$2

fail() {
    echo "SMOKE TEST FAILED: $*" >&2
    exit 1
}

# The version, and the IR contract version it carries.
reported=$("$brygge" --version) || fail "brygge --version failed"
printf '%s\n' "$reported"
first=$(printf '%s\n' "$reported" | sed -n 1p)
[ "$first" = "brygge $version" ] || fail "expected 'brygge $version', got '$first'"
printf '%s\n' "$reported" | grep -qE '^IR contract version: [0-9]+\.[0-9]+\.[0-9]+$' || fail "no 'IR contract version: X.Y.Z' line"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# A two-commit repository, with a fixed identity and no ambient Git configuration.
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null
export GIT_AUTHOR_NAME="Smoke Test" GIT_AUTHOR_EMAIL="smoke@example.com"
export GIT_COMMITTER_NAME="Smoke Test" GIT_COMMITTER_EMAIL="smoke@example.com"
repo="$work/repo"
git init -q --initial-branch=main "$repo"
git -C "$repo" config commit.gpgsign false
printf 'one\n' >"$repo/a.txt"
git -C "$repo" add a.txt
git -C "$repo" commit -q -m "first"
printf 'two\n' >>"$repo/a.txt"
printf 'new\n' >"$repo/b.txt"
git -C "$repo" add a.txt b.txt
git -C "$repo" commit -q -m "second"

artifact="$work/history.brygge"
"$brygge" decode git "$repo" --out "$artifact" --format machine >"$work/decode.txt" || fail "decode exited $?"
[ -s "$artifact" ] || fail "decode wrote no artifact"
grep -qx 'atoms=2' "$work/decode.txt" || fail "decode did not report two atoms: $(cat "$work/decode.txt")"
echo "decode: exit 0, 2 atoms"

"$brygge" verify "$artifact" --format machine >"$work/verify.txt" || fail "verify exited $?"
grep -qx 'verify.result=pass' "$work/verify.txt" || fail "verify did not pass: $(cat "$work/verify.txt")"
echo "verify: exit 0, pass"

"$brygge" verify "$artifact" --against-source "$repo" --format machine >"$work/against.txt" || fail "verify --against-source exited $?"
grep -qx 'verify.result=pass' "$work/against.txt" || fail "verify --against-source did not pass: $(cat "$work/against.txt")"
echo "verify --against-source: exit 0, pass"

echo "OK: brygge $version decodes, verifies, and verifies against source a two-commit Git repository"
