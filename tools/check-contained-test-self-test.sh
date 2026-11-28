#!/usr/bin/env bash
# P-5's self-test (RFC 022 incident 094): proves `tools/contained-test.sh`'s own containment actually
# stops a runaway, rather than trusting that it would. **CI only — HANDOFF.md §7a's first row: this
# starts up to 300 real processes, which this machine must never do outside a bound CI already applies
# to the whole job.** Never run this locally.
#
# It picks the same containment `tools/contained-test.sh` would (systemd-run's cgroup scope, or the
# `ulimit` fallback), then tries to start 300 background `sleep` processes under it — a hard cap this
# script itself enforces (the loop below always attempts exactly 300, succeed or not, so this self-test
# can never itself exceed the limit it is testing), and counts how many actually ended up alive.
#
# Usage: tools/check-contained-test-self-test.sh
set -uo pipefail # not -e: a refused spawn, or the shell's own fork-retry noise, is expected, not a bug

cd "$(dirname "${BASH_SOURCE[0]}")/.."

# Attempts exactly 300 spawns, **never stopping early**: a single failed `fork()` does not reliably
# leave `$!` empty or unset (a review of this self-test's first CI run found it can instead leave `$!`
# holding an *earlier, still-running* pid, which a per-iteration `kill -0 "$!"` check then misreads as
# "this spawn succeeded" — exactly the false pass an in-loop check risks). This counts ground truth
# instead, once, at the end: of every `$!` this loop ever saw, how many are *distinct and currently
# alive*. Bash's own stderr noise while fork() is failing ("fork: retry: Resource temporarily
# unavailable", then "fork: Resource temporarily unavailable") is cosmetic here and never inspected —
# the live-process count afterward is the only thing that decides the result.
#
# Kills everything it started before returning, so nothing outlives this function (relevant for the
# `ulimit` branch, which runs this in a subshell of this very script; the `systemd-run` branch's own
# scope is torn down by systemd once its process exits, independently of this).
spawn_up_to_300() {
    local attempted=()
    local i newpid
    for ((i = 0; i < 300; i++)); do
        sleep 30 &
        newpid=$!
        [ -n "$newpid" ] && attempted+=("$newpid")
    done

    local -A seen=()
    local alive_count=0
    local p
    for p in "${attempted[@]}"; do
        if [ -z "${seen[$p]:-}" ]; then
            seen[$p]=1
            kill -0 "$p" 2>/dev/null && alive_count=$((alive_count + 1))
        fi
    done

    for p in "${!seen[@]}"; do
        kill "$p" >/dev/null 2>&1 || true
    done

    echo "$alive_count distinct process(es) actually alive after 300 spawn attempts"
    [ "$alive_count" -lt 300 ]
}

if command -v systemd-run >/dev/null 2>&1 &&
    systemd-run --user --scope -p TasksMax=8 -p MemoryMax=64M -- true >/dev/null 2>&1; then
    echo "self-test: systemd-run --user --scope -p TasksMax=256 -p MemoryMax=8G"
    # `spawn_up_to_300` runs *inside* the scope under test, as that one scope's own task count — not in
    # this script's own shell, which the scope does not bound. `declare -f` re-serializes the function
    # so the child shell `systemd-run` starts has it, without re-sourcing this file.
    if systemd-run --user --scope -p TasksMax=256 -p MemoryMax=8G -- \
        bash -c "$(declare -f spawn_up_to_300); spawn_up_to_300"; then
        echo "OK: the systemd-run scope's TasksMax=256 refused some processes before 300"
    else
        echo "FAILED: all 300 processes started under the systemd-run scope's TasksMax=256" >&2
        exit 1
    fi
else
    echo "self-test: ulimit -u 512 -v 8000000 (systemd-run unavailable or not usable here)"
    if (
        ulimit -u 512 -v 8000000
        spawn_up_to_300
    ); then
        echo "OK: ulimit -u 512 refused some processes before 300"
    else
        echo "FAILED: all 300 processes started under ulimit -u 512" >&2
        exit 1
    fi
fi
