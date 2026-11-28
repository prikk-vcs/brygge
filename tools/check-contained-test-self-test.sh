#!/usr/bin/env bash
# P-5's self-test (RFC 022 incident 094): proves `tools/contained-test.sh`'s own containment actually
# stops a runaway, rather than trusting that it would. **CI only — HANDOFF.md §7a's first row: this
# starts up to 300 real processes, which this machine must never do outside a bound CI already applies
# to the whole job.** Never run this locally.
#
# It picks the same containment `tools/contained-test.sh` would (systemd-run's cgroup scope, or the
# `ulimit` fallback), then tries, *inside* that containment, to start 300 background `sleep` processes —
# a hard cap this script itself enforces (the loop below always attempts exactly 300, succeed or not, so
# this self-test can never itself exceed the limit it is testing) — and counts how many actually ended
# up alive.
#
# Two earlier versions of this detection logic were both wrong, for related reasons, and both are
# recorded here so a third rewrite does not repeat either:
#   1. Checking each spawn's success in-loop via `kill -0 "$!"` misreads a failed `fork()` as success
#      whenever `$!` happens to still hold an *earlier, still-running* pid (a failed fork does not
#      reliably clear `$!`).
#   2. Passing the fix for (1) — a running tally, kept in bash arrays local to the spawning loop — across
#      the `systemd-run --scope -- bash -c ...` process boundary by serializing the loop as a function
#      with `declare -f` broke in a different way: an array *expansion* such as `${attempted[@]}` is not
#      something a plain environment variable or a second shell's `-c` argument can carry intact, and
#      systemd-run's own argument handling misread fragments of it as an (invalid) environment variable
#      assignment.
# Neither mistake is possible with the mechanism below: **no shell array crosses any process boundary,
# and no in-loop check decides anything.** The spawning loop is a short, self-contained command string
# with no local state beyond a loop counter; after each attempt it appends that one attempt's `$!` to a
# plain file, one pid per line — plain text written to disk, not shell state, is what survives the
# `systemd-run`/subshell boundary correctly. Once the loop (started inside the containment) has
# finished or been cut short, *this* script — running outside that containment — reads the file,
# de-duplicates it, and checks with its own `kill -0` which of those pids are actually alive right now.
# That ground-truth count, never anything read mid-loop, is the only thing the pass/fail decision rests
# on.
#
# Usage: tools/check-contained-test-self-test.sh
set -uo pipefail # not -e: a refused spawn, or the shell's own fork-retry noise, is expected, not a bug

cd "$(dirname "${BASH_SOURCE[0]}")/.."

progress_file=$(mktemp)
cleanup() { rm -f "$progress_file"; }
trap cleanup EXIT

# A plain command string, not a function: nothing here is passed to the other side as shell state, only
# as this literal text, interpreted fresh by whichever bash runs it. `$PROGRESS_FILE` is read from that
# process's own environment (set explicitly below for each containment form), never interpolated into
# this string, so no quoting of the path can go wrong here.
spawn_loop='for ((i = 0; i < 300; i++)); do sleep 30 & echo "$!" >>"$PROGRESS_FILE"; done'

if command -v systemd-run >/dev/null 2>&1 &&
    systemd-run --user --scope -p TasksMax=8 -p MemoryMax=64M -- true >/dev/null 2>&1; then
    echo "self-test: systemd-run --user --scope -p TasksMax=256 -p MemoryMax=8G"
    systemd-run --user --scope -p TasksMax=256 -p MemoryMax=8G \
        --setenv="PROGRESS_FILE=$progress_file" -- bash -c "$spawn_loop"
else
    echo "self-test: ulimit -u 512 -v 8000000 (systemd-run unavailable or not usable here)"
    (
        export PROGRESS_FILE="$progress_file"
        ulimit -u 512 -v 8000000
        bash -c "$spawn_loop"
    )
fi

mapfile -t candidates < <(sort -u "$progress_file" | grep -E '^[0-9]+$')
alive_count=0
for pid in "${candidates[@]:-}"; do
    [ -n "$pid" ] || continue
    if kill -0 "$pid" 2>/dev/null; then
        alive_count=$((alive_count + 1))
        kill "$pid" >/dev/null 2>&1 || true
    fi
done

echo "$alive_count distinct process(es) actually alive after 300 spawn attempts"
if [ "$alive_count" -lt 300 ]; then
    echo "OK: the containment refused some processes before 300"
else
    echo "FAILED: all 300 processes started" >&2
    exit 1
fi
