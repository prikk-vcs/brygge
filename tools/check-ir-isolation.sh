#!/usr/bin/env bash
# Enforce brygge-ir's dependency isolation as a tested property, not an asserted one (RFC 009 D-7,
# CR-12.1): brygge-ir is the light core a target links, and it must never gain a decoder or any other
# dependency without an architect review. Fails if brygge-ir's normal dependency closure differs from
# crates/brygge-ir/allowed-dependencies.txt.
#
# Runs with `--target all` (review 092): brygge ships aarch64 binaries for Linux and macOS, and some
# dependencies (e.g. `libc`, pulled in by `cpufeatures` only off x86/x86_64) are architecture-conditional.
# `--target all` checks every platform's closure at once, from any single runner, so this script's result
# does not depend on which architecture it happens to run on.
# Usage: tools/check-ir-isolation.sh
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

allowlist="crates/brygge-ir/allowed-dependencies.txt"

actual=$(cargo tree -p brygge-ir -e normal --target all --prefix none --format '{p}' \
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

# RFC 019's acceptance refinement: crates/brygge/src/encode_git/ (the built-in `brygge encode git`
# encoder) is isolated the same way brygge-ir itself is, but it is a *module* of the `brygge` crate, not
# a crate `cargo tree` can check on its own — so this is a source-level scan instead, over every `use`
# statement in that directory: its root must be one of brygge_ir, std, core, alloc, super, or
# crate::encode_git (never a bare crate::<anything else>, a decoder, gix, or any other crate).
encode_git_dir="crates/brygge/src/encode_git"
if [ -d "$encode_git_dir" ]; then
    # Rust 2018's "uniform paths": `use trailers::X;` for a sibling submodule declared with
    # `mod trailers;` in the same file resolves as `self::trailers`, with no `self::`/`crate::` prefix
    # needed — so a local submodule's own name is as allowed a root as `super` or `self`.
    local_modules=$(grep -hoE '^[[:space:]]*(pub(\(crate\))?[[:space:]]+)?mod[[:space:]]+[a-zA-Z0-9_]+' \
        "$encode_git_dir"/*.rs "$encode_git_dir"/**/*.rs 2>/dev/null \
        | sed -E 's/^.*mod[[:space:]]+//' | sort -u)
    violations=""
    while IFS= read -r -d '' f; do
        while IFS=: read -r lineno line; do
            path=$(echo "$line" | sed -E 's/^[[:space:]]*(pub(\(crate\))?[[:space:]]+)?use[[:space:]]+//; s/[;{].*$//; s/[[:space:]]+$//')
            root=$(echo "$path" | cut -d: -f1)
            second=$(echo "$path" | awk -F'::' '{print $2}')
            case "$root" in
                std | core | alloc | super | self | brygge_ir) ;;
                crate)
                    if [ "$second" != "encode_git" ]; then
                        violations="$violations
$f:$lineno: $line"
                    fi
                    ;;
                *)
                    if ! echo "$local_modules" | grep -qxF "$root"; then
                        violations="$violations
$f:$lineno: $line"
                    fi
                    ;;
            esac
        done < <(grep -nE '^[[:space:]]*(pub(\(crate\))?[[:space:]]+)?use[[:space:]]+' "$f")
    done < <(find "$encode_git_dir" -name '*.rs' -print0)
    if [ -n "$violations" ]; then
        echo "FAILED: $encode_git_dir imports outside its allowed closure" \
            "(brygge_ir, std, core, alloc, super, crate::encode_git):"
        echo "$violations"
        exit 1
    fi
    echo "OK: $encode_git_dir imports nothing outside brygge_ir/std/core/alloc/super/crate::encode_git."
else
    echo "skip: $encode_git_dir does not exist yet"
fi
