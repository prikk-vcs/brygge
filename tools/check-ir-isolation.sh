#!/usr/bin/env bash
# Enforce brygge-ir's dependency isolation as a tested property, not an asserted one (RFC 009 D-7,
# CR-12.1): brygge-ir is the light core a target links, and it must never gain a decoder or any other
# dependency without an architect review. Fails if brygge-ir's normal dependency closure differs from
# crates/brygge-ir/allowed-dependencies.txt.
# Usage: tools/check-ir-isolation.sh
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

allowlist="crates/brygge-ir/allowed-dependencies.txt"

actual=$(cargo tree -p brygge-ir -e normal --prefix none --format '{p}' \
    | sed -E 's/^([a-zA-Z0-9_-]+) .*/\1/' \
    | sort -u)
expected=$(grep -v '^#' "$allowlist" | grep -v '^\s*$' | sort -u)

if [ "$actual" != "$expected" ]; then
    echo "FAILED: brygge-ir's dependency closure does not match $allowlist"
    echo
    echo "--- expected (from $allowlist) ---"
    echo "$expected"
    echo
    echo "--- actual (from cargo tree) ---"
    echo "$actual"
    echo
    echo "diff (expected vs actual):"
    diff <(echo "$expected") <(echo "$actual") || true
    echo
    echo "If this is a deliberate, architect-reviewed addition, update $allowlist. Otherwise, brygge-ir"
    echo "has gained a dependency it must not have (RFC 009 D-7)."
    exit 1
fi

echo "OK: brygge-ir's dependency closure matches $allowlist ($(echo "$expected" | wc -l) crate(s))."
