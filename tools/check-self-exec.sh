#!/usr/bin/env bash
# P-3 (RFC 022 incident 094): a program that re-runs its own executable (`std::env::current_exe()`) is
# dangerous in a way a decoder or a plugin lookup is not — under `cargo test`, `current_exe()` is the
# *test harness*, not the program under test, and a harness run with no arguments runs its whole default
# test set again. That is exactly how the pre-incident watchdog became a fork bomb: a unit test called a
# function that spawned `current_exe()`, and the spawned "child" was the same test binary, which ran the
# same test again, which spawned another child, unbounded.
#
# This script enforces HANDOFF.md §7a's rule: "a program that re-runs itself is allowed only at a site
# listed in tools/check-self-exec.sh, which names each site's guard." It is a count, not a line-number
# pin (line numbers drift on an unrelated edit; a changed *count* is the thing worth failing on): for
# each reviewed file below, it counts how many *real code* lines (comments excluded) contain any of the
# patterns a program can use to find its own executable — `current_exe(`, `args().next()`,
# `args_os().next()`, `.nth(0)`, or `/proc/self/exe` (review 205, correcting 204's narrower search) —
# and fails if that count differs from what is on record — whether the count rose (an unreviewed new
# site) or fell (the list is stale and should be tightened). Any file containing one of these patterns
# that is not in this list at all fails outright.
#
# Known blind spot (review 205, small point): this is a count per file, not an identity check on each
# site. One self-exec site being swapped for another in the same file, while the total count for that
# file stays the same, passes unnoticed. Accepted as a tradeoff — not fixed here.
#
# Usage: tools/check-self-exec.sh
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

# --- the reviewed list --------------------------------------------------------------------------------
# file<TAB>expected-count<TAB>guard note
reviewed=$(cat <<'EOF'
crates/brygge-ir/src/artifact/tests.rs	1	review 103 P-3: re-run filtered with `--exact` to one named helper test, guarded by DEFLATE_BOMB_CHILD_ENV; the harness can never re-enter the parent test
crates/brygge/src/watchdog.rs	1	P-1: only `main` calls `start`; refused under the marker; compiled out under `cfg(test)`; `ready` handshake within 2 s
tools/bench/src/main.rs	1	P-2: spawn_self_run, #[cfg(not(test))] only, refuses when BENCH_CHILD_ENV is already set
tools/bench/src/profile.rs	1	P-2: run() learns this process's own path unconditionally (harmless by itself: only the later spawn is dangerous); spawn_self_profile_one, #[cfg(not(test))] only, takes that path as a parameter and does the actual spawning, refusing when BENCH_CHILD_ENV is already set
tools/bench/src/real.rs	2	P-2 (spawn_self_decode_one, #[cfg(not(test))] only, refuses when BENCH_CHILD_ENV is already set); the second site is brygge_bin(), which only locates the sibling `brygge` binary's path and spawns nothing of its own
tools/transform-test-plugin/src/main.rs	1	review 205 R-5: name only, no spawn
EOF
)

fail=0

# --- the patterns a program can use to find its own executable (review 205 R-5) ----------------------
self_exec_patterns=('current_exe(' 'args().next()' 'args_os().next()' '.nth(0)' '/proc/self/exe')

matches_self_exec() {
    local line="$1" pat
    for pat in "${self_exec_patterns[@]}"; do
        [[ "$line" == *"$pat"* ]] && return 0
    done
    return 1
}

# --- count every real-code self-exec occurrence, file by file ------------------------------------------
actual_files=$(grep -rlF -e 'current_exe(' -e 'args().next()' -e 'args_os().next()' -e '.nth(0)' \
    -e '/proc/self/exe' crates/ tools/ examples/ --include='*.rs' 2>/dev/null | sort -u || true)

for f in $actual_files; do
    count=0
    while IFS= read -r line; do
        trimmed="${line#"${line%%[![:space:]]*}"}" # strip leading whitespace
        case "$trimmed" in
        //*) continue ;; # a `//`/`///`/`//!` comment line is prose, not code
        esac
        if matches_self_exec "$line"; then
            count=$((count + 1))
        fi
    done <"$f"
    [ "$count" -eq 0 ] && continue # every match was in a comment; nothing to list

    expected=$(echo "$reviewed" | awk -F'\t' -v f="$f" '$1 == f { print $2 }')
    if [ -z "$expected" ]; then
        echo "FAILED: $f has $count real self-exec call(s), and is not in tools/check-self-exec.sh's" \
            "reviewed list at all"
        fail=1
        continue
    fi
    if [ "$count" -ne "$expected" ]; then
        echo "FAILED: $f has $count real self-exec call(s), but the reviewed list expects $expected" \
            "— a site was added, removed, or the list is stale"
        fail=1
    fi
done

# --- every reviewed file with a nonzero expectation must actually have that many -----------------------
while IFS=$'\t' read -r f expected _note; do
    [ "$expected" -eq 0 ] && continue
    if ! echo "$actual_files" | grep -qxF "$f"; then
        echo "FAILED: tools/check-self-exec.sh's reviewed list expects $expected real self-exec" \
            "call(s) in $f, but that file has none (or does not exist) — the list is stale"
        fail=1
    fi
done <<<"$reviewed"

if [ "$fail" -ne 0 ]; then
    echo
    echo "A program that re-runs itself (current_exe(), args().next()/args_os().next()/.nth(0), or" \
        "/proc/self/exe) is allowed only at a site named in tools/check-self-exec.sh's own reviewed" \
        "list (HANDOFF.md §7a). Review the new or changed site, add it with its guard, or revert it."
    exit 1
fi

echo "OK: every real self-exec call matches tools/check-self-exec.sh's reviewed list."
