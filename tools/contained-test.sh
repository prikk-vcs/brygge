#!/usr/bin/env bash
# P-5 (RFC 022 incident 094, HANDOFF.md §7a): every local test run uses this instead of bare
# `cargo test`. A test that spawns more than the limit's share of processes, or allocates past its
# share of memory, fails *inside* the limit here — instead of taking the machine down, as the
# pre-incident watchdog's fork bomb did.
#
# It tries `systemd-run --user --scope` first (a real cgroup bound: at most 256 tasks, 8 GiB of memory
# with no swap, for the whole contained run, processes and threads together); where that is
# unavailable, or where it is accepted but not actually enforced (review 205 R-2, below), a subshell
# under `ulimit -u 512 -v 8000000` (processes, and virtual memory in KiB — no cgroup, so no memory.swap
# control either, but this machine's swap is zram, compressed RAM, so a ulimit'd process's own growth is
# still bounded well short of it). It states which it used, and why, on its first line of output, and
# never runs unlimited either way.
#
# review 205 R-1: `MemoryMax` alone does not bound swap — above it, the kernel swaps a cgroup out
# instead of stopping it, and this machine's 62 GB zram swap device means a leaking test could still
# reach the incident's RAM-and-swap-both-above-90% state. `-p MemorySwapMax=0` makes `MemoryMax` a real
# ceiling: memory.swap.max is set to 0, so there is nothing to swap into.
#
# review 205 R-2: `systemd-run` accepting these properties only proves it understood the syntax — not
# that a cgroup controller enforcing them is actually delegated to this user's manager. The probe below
# therefore reads back its own scope's pids.max, memory.max and memory.swap.max and compares each to
# what it asked for; a mismatch on any of the three means the limit would silently go unenforced, so the
# ulimit fallback is used instead, and the mismatch is named on the first line of output.
#
# P-6 (RFC 022 incident 094): lists the system temporary directory's brygge-named entries before and
# after the run, and fails if any were added — the test suite must remove what it creates there.
#
# Usage: tools/contained-test.sh <cargo test arguments...>
# Example: tools/contained-test.sh --workspace --locked --no-fail-fast
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

tmp_dir=${TMPDIR:-/tmp}

# review 205 R-2: the probe's own cgroup readback has to leave the probe's scope as plain text on disk,
# the same way the self-test's own progress file already crosses the `systemd-run` boundary (review
# 204/205's shared lesson: shell state does not survive that boundary, a file does) — the scope, and the
# shell running inside it, are both gone by the time this script can next inspect anything.
probe_file=$(mktemp)
cleanup() { rm -f "$probe_file"; }
trap cleanup EXIT

# A plain command string, not a function, for the same reason check-contained-test-self-test.sh uses
# one: nothing here crosses the systemd-run boundary as shell state, only as this literal text.
probe_read_back='
cg="/sys/fs/cgroup$(cut -d: -f3 /proc/self/cgroup)"
{
    cat "$cg/pids.max" 2>/dev/null || echo unknown
    cat "$cg/memory.max" 2>/dev/null || echo unknown
    cat "$cg/memory.swap.max" 2>/dev/null || echo unknown
} >"$PROBE_FILE"
'

use_systemd_run=0
enforcement_mismatch=""

if command -v systemd-run >/dev/null 2>&1 &&
    systemd-run --user --scope -p TasksMax=8 -p MemoryMax=64M -p MemorySwapMax=0 \
        --setenv="PROBE_FILE=$probe_file" -- bash -c "$probe_read_back" >/dev/null 2>&1; then
    probed_pids=$(sed -n '1p' "$probe_file" 2>/dev/null || echo unknown)
    probed_mem=$(sed -n '2p' "$probe_file" 2>/dev/null || echo unknown)
    probed_swap=$(sed -n '3p' "$probe_file" 2>/dev/null || echo unknown)
    expected_mem=$((64 * 1024 * 1024)) # 64M, as systemd resolves byte-size suffixes: 1024-based

    reasons=""
    [ "$probed_pids" = "8" ] || reasons="${reasons}pids.max=$probed_pids, "
    [ "$probed_mem" = "$expected_mem" ] || reasons="${reasons}memory.max=$probed_mem, "
    [ "$probed_swap" = "0" ] || reasons="${reasons}memory.swap.max=$probed_swap, "

    if [ -z "$reasons" ]; then
        use_systemd_run=1
    else
        enforcement_mismatch="${reasons%, }"
    fi
fi

# P-6: a snapshot of what brygge's own test suite might have left behind in the system temp directory,
# taken before this run so a later diff names only what this run itself added. The code matches every
# `brygge*` glob — not just the specific forms brygge's own tests and tools are known to name
# (`brygge-cli-*`, `brygge-atomic-*`, `brygge-bench-*`) — because the point of the check is to notice a
# new one too, named or not.
snapshot() {
    find "$tmp_dir" -maxdepth 1 -name 'brygge*' 2>/dev/null | sort
}

before=$(snapshot)

status=0
if [ "$use_systemd_run" -eq 1 ]; then
    echo "contained-test: systemd-run --user --scope (TasksMax=256, MemoryMax=8G, MemorySwapMax=0 — read back and verified)"
    systemd-run --user --scope -p TasksMax=256 -p MemoryMax=8G -p MemorySwapMax=0 -- \
        cargo test "$@" || status=$?
else
    if [ -n "$enforcement_mismatch" ]; then
        echo "contained-test: ulimit -u 512 -v 8000000 (systemd-run's limits were not enforced: $enforcement_mismatch)"
    else
        echo "contained-test: ulimit -u 512 -v 8000000 (systemd-run unavailable or not usable here)"
    fi
    (
        ulimit -u 512 -v 8000000
        cargo test "$@"
    ) || status=$?
fi

after=$(snapshot)
leaked=$(comm -13 <(echo "$before") <(echo "$after"))
if [ -n "$leaked" ]; then
    echo "FAILED (P-6, incident 094): the test suite left temporary files behind in $tmp_dir:" >&2
    echo "$leaked" >&2
    exit 1
fi

exit "$status"
