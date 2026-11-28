#!/usr/bin/env bash
# P-5's self-test (RFC 022 incident 094): proves `tools/contained-test.sh`'s own containment actually
# stops a runaway, rather than trusting that it would. **CI only — HANDOFF.md §7a's first row: this
# starts up to 300 real processes, which this machine must never do outside a bound CI already applies
# to the whole job.** Never run this locally.
#
# It picks the same containment `tools/contained-test.sh` would (systemd-run's cgroup scope, or the
# `ulimit` fallback), then tries to start up to 300 background `sleep` processes under it — a hard cap
# this script itself enforces (the loop below, bounded at 300, runs regardless of whether the
# containment underneath it works, so this self-test can never itself exceed the limit it is testing).
# `spawn_up_to_300` exits 0 the moment a spawn is refused (the containment worked, well before 300) and
# exits 1 if it reaches 300 with nothing ever refused (the containment did not bind anything) — the one
# thing this script checks is that exit status, under each containment form in turn. Everything spawned
# is killed before this script exits, successful or not.
#
# Usage: tools/check-contained-test-self-test.sh
set -uo pipefail # not -e: a spawn failure past the limit is the expected, asserted outcome, not a bug

cd "$(dirname "${BASH_SOURCE[0]}")/.."

pids=()
cleanup() {
    for pid in "${pids[@]:-}"; do
        kill "$pid" >/dev/null 2>&1 || true
    done
}
trap cleanup EXIT

# Exits 0 (refused before the cap) or 1 (reached the cap unrefused). Prints which, for the log.
# `$!` is checked for both non-empty and actually alive (`kill -0`), not trusted alone: if the shell's
# own fork for `&` fails, `$!` can be left unset or stale, and a bare `[ -n "$!" ]` would not catch that.
spawn_up_to_300() {
    local started=0
    local i newpid
    for ((i = 0; i < 300; i++)); do
        sleep 30 &
        newpid=$!
        if [ -n "$newpid" ] && kill -0 "$newpid" 2>/dev/null; then
            pids+=("$newpid")
            started=$((started + 1))
        else
            echo "refused at spawn #$((i + 1)) (after $started succeeded)"
            return 0
        fi
    done
    echo "reached 300 with none refused"
    return 1
}

if command -v systemd-run >/dev/null 2>&1 &&
    systemd-run --user --scope -p TasksMax=8 -p MemoryMax=64M -- true >/dev/null 2>&1; then
    echo "self-test: systemd-run --user --scope -p TasksMax=256 -p MemoryMax=8G"
    # `spawn_up_to_300` runs *inside* the scope under test, as that one scope's own task count — not in
    # this script's own shell, which the scope does not bound. `declare -f` re-serializes the function
    # so the child shell `systemd-run` starts has it, without re-sourcing this file.
    if systemd-run --user --scope -p TasksMax=256 -p MemoryMax=8G -- \
        bash -c "$(declare -f spawn_up_to_300); spawn_up_to_300"; then
        echo "OK: the systemd-run scope's TasksMax=256 refused further processes before 300"
    else
        echo "FAILED: the systemd-run scope never refused a spawn before 300 processes" >&2
        exit 1
    fi
else
    echo "self-test: ulimit -u 512 -v 8000000 (systemd-run unavailable or not usable here)"
    if (
        ulimit -u 512 -v 8000000
        spawn_up_to_300
    ); then
        echo "OK: ulimit -u 512 refused further processes before 300"
    else
        echo "FAILED: ulimit -u 512 never refused a spawn before 300 processes" >&2
        exit 1
    fi
fi
