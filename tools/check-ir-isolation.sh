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

    # Review 136 F-6: the check above reads only `use` lines, so a fully qualified path with no `use` at
    # all (an inline `crate::plugin::find(...)` call, or `brygge_decode_git::decode(...)`) would bypass
    # it. Scan every non-comment line in the directory instead, for: a bare `crate::` not followed by
    # `encode_git`; the root of any *direct* dependency of the `brygge` crate other than `brygge_ir`
    # (from `crates/brygge/Cargo.toml`'s own `[dependencies]`, hyphens turned into underscores — a crate
    # `brygge` does not directly depend on cannot be named as a path root at all, so this list is already
    # exhaustive); and `extern crate`. A `//`/`///`/`//!` line comment is skipped; a trailing comment on
    # an otherwise-real code line, or a block comment, is not — this is a text scan, not a parser.
    forbidden_deps=$(awk '/^\[dependencies\]/{f=1; next} /^\[/{f=0} f && /^[a-zA-Z0-9_-]+[[:space:]]*=/ {
        sub(/[[:space:]]*=.*/, ""); gsub(/-/, "_"); if ($0 != "brygge_ir") print
    }' crates/brygge/Cargo.toml | sort -u)
    deep_violations=""
    while IFS= read -r -d '' f; do
        while IFS=: read -r lineno line; do
            trimmed=$(echo "$line" | sed -E 's/^[[:space:]]*//')
            case "$trimmed" in
                //*) continue ;;
            esac
            if echo "$line" | grep -qE '(^|[^a-zA-Z0-9_:])crate::' && \
                ! echo "$line" | grep -qE '(^|[^a-zA-Z0-9_:])crate::encode_git([^a-zA-Z0-9_]|$)'; then
                deep_violations="$deep_violations
$f:$lineno: $line"
                continue
            fi
            if echo "$line" | grep -qE '(^|[^a-zA-Z0-9_])extern[[:space:]]+crate([^a-zA-Z0-9_]|$)'; then
                deep_violations="$deep_violations
$f:$lineno: $line"
                continue
            fi
            for dep in $forbidden_deps; do
                if echo "$line" | grep -qE "(^|[^a-zA-Z0-9_])${dep}::"; then
                    deep_violations="$deep_violations
$f:$lineno: $line"
                    break
                fi
            done
        done < <(grep -n '.' "$f")
    done < <(find "$encode_git_dir" -name '*.rs' -print0)
    if [ -n "$deep_violations" ]; then
        echo "FAILED: $encode_git_dir has a fully qualified path outside its allowed closure" \
            "(a bare crate::<x> other than crate::encode_git, a brygge dependency other than" \
            "brygge_ir, or extern crate):"
        echo "$deep_violations"
        exit 1
    fi
    echo "OK: $encode_git_dir has no fully qualified path outside its allowed closure either."
else
    echo "skip: $encode_git_dir does not exist yet"
fi
