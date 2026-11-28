#!/usr/bin/env bash
# P-5's self-test (RFC 022 incident 094): proves `tools/contained.sh`'s own containment actually
# stops a runaway, rather than trusting that it would. **CI only — HANDOFF.md §7a's first row: this
# starts up to 300 real processes, which this machine must never do outside a bound CI already applies
# to the whole job.** Never run this locally.
#
# It picks the same containment `tools/contained.sh` would (systemd-run's cgroup scope, read back
# and verified exactly as that script does — review 205 R-2, below — or the `ulimit` fallback), then
# tries, *inside* that containment, to start 300 background `sleep` processes — a hard cap this script
# itself enforces (the loop below always attempts exactly 300, succeed or not, so this self-test can
# never itself exceed the limit it is testing) — and counts how many actually ended up alive. Where the
# cgroup scope is confirmed real, it then also proves the *memory* bound, by holding more memory than a
# small scope allows and checking that the scope's own cgroup recorded an OOM kill (review 205 R-3).
#
# review 205 R-1: `-p MemorySwapMax=0` on every scope below, for the same reason as
# `tools/contained.sh`: without it, `MemoryMax` alone is not a real ceiling on this machine's zram
# swap.
#
# review 205 R-2: the probe below reads back its own scope's pids.max, memory.max, memory.swap.max and
# (review 208 R-7) cpu.max, and compares each to what it asked for, the same way `tools/contained.sh`'s
# probe does, and falls back to `ulimit` on any mismatch rather than trusting that `systemd-run` accepting
# the properties means they are enforced.
#
# Two earlier versions of the *process-count* detection logic were both wrong, for related reasons, and
# both are recorded here so a third rewrite does not repeat either:
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
# on. The same file-not-shell-state discipline carries the memory proof's own `memory.events` reading
# out of its scope below, and the R-2 probe's cgroup readback out of its scope above that.
#
# Usage: tools/check-contained-test-self-test.sh
set -uo pipefail # not -e: a refused spawn, or the shell's own fork-retry noise, is expected, not a bug

cd "$(dirname "${BASH_SOURCE[0]}")/.."

progress_file=$(mktemp)
probe_file=$(mktemp)
mem_events_file=$(mktemp)
cleanup() { rm -f "$progress_file" "$probe_file" "$mem_events_file"; }
trap cleanup EXIT

# review 205 R-2: the same probe-and-read-back `tools/contained.sh` uses, so this script picks the
# identical containment that script would, rather than trusting `systemd-run`'s syntax acceptance alone.
# review 208 R-7: `cpu.max` is read back too, against the same quarter-of-`nproc` (minimum 2) CPUQuota
# `tools/contained.sh` computes.
probe_read_back='
cg="/sys/fs/cgroup$(cut -d: -f3 /proc/self/cgroup)"
{
    cat "$cg/pids.max" 2>/dev/null || echo unknown
    cat "$cg/memory.max" 2>/dev/null || echo unknown
    cat "$cg/memory.swap.max" 2>/dev/null || echo unknown
    cat "$cg/cpu.max" 2>/dev/null || echo unknown
} >"$PROBE_FILE"
'

cores=$(nproc)
cpu_n=$((cores / 4))
if [ "$cpu_n" -lt 2 ]; then
    cpu_n=2
fi
expected_cpu="$((cpu_n * 100000)) 100000"

use_systemd_run=0
enforcement_mismatch=""

if command -v systemd-run >/dev/null 2>&1 &&
    systemd-run --user --scope -p TasksMax=8 -p MemoryMax=64M -p MemorySwapMax=0 \
        -p "CPUQuota=$((cpu_n * 100))%" \
        --setenv="PROBE_FILE=$probe_file" -- bash -c "$probe_read_back" >/dev/null 2>&1; then
    probed_pids=$(sed -n '1p' "$probe_file" 2>/dev/null || echo unknown)
    probed_mem=$(sed -n '2p' "$probe_file" 2>/dev/null || echo unknown)
    probed_swap=$(sed -n '3p' "$probe_file" 2>/dev/null || echo unknown)
    probed_cpu=$(sed -n '4p' "$probe_file" 2>/dev/null || echo unknown)
    expected_mem=$((64 * 1024 * 1024)) # 64M, as systemd resolves byte-size suffixes: 1024-based

    reasons=""
    [ "$probed_pids" = "8" ] || reasons="${reasons}pids.max=$probed_pids, "
    [ "$probed_mem" = "$expected_mem" ] || reasons="${reasons}memory.max=$probed_mem, "
    [ "$probed_swap" = "0" ] || reasons="${reasons}memory.swap.max=$probed_swap, "
    [ "$probed_cpu" = "$expected_cpu" ] || reasons="${reasons}cpu.max=$probed_cpu, "

    if [ -z "$reasons" ]; then
        use_systemd_run=1
    else
        enforcement_mismatch="${reasons%, }"
    fi
fi

# A plain command string, not a function: nothing here is passed to the other side as shell state, only
# as this literal text, interpreted fresh by whichever bash runs it. `$PROGRESS_FILE` is read from that
# process's own environment (set explicitly below for each containment form), never interpolated into
# this string, so no quoting of the path can go wrong here.
spawn_loop='for ((i = 0; i < 300; i++)); do sleep 30 & echo "$!" >>"$PROGRESS_FILE"; done'

if [ "$use_systemd_run" -eq 1 ]; then
    echo "self-test: systemd-run --user --scope -p TasksMax=256 -p MemoryMax=8G -p MemorySwapMax=0 (read back and verified)"
    systemd-run --user --scope -p TasksMax=256 -p MemoryMax=8G -p MemorySwapMax=0 \
        --setenv="PROGRESS_FILE=$progress_file" -- bash -c "$spawn_loop"
else
    if [ -n "$enforcement_mismatch" ]; then
        echo "self-test: ulimit -u 512 -v 8000000 (systemd-run's limits were not enforced: $enforcement_mismatch)"
    else
        echo "self-test: ulimit -u 512 -v 8000000 (systemd-run unavailable or not usable here)"
    fi
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

fail=0

echo "$alive_count distinct process(es) actually alive after 300 spawn attempts"
# review 205 R-3: tightened from "fewer than 300" to "at most 256" — the actual TasksMax bound this
# script and tools/contained-test.sh both advertise. 300 remains only the number of spawns attempted.
if [ "$alive_count" -le 256 ]; then
    echo "OK: the containment held at or under its 256-process bound (process proof)"
else
    echo "FAILED: $alive_count processes were alive, above the 256-process bound being checked" >&2
    fail=1
fi

# review 205 R-3: the process proof above shows only that TasksMax is enforced; it says nothing about
# MemoryMax. This holds about 512 MB of real memory under a 256M cap (small, so cheap in CI) and checks
# that the scope's own cgroup recorded an OOM kill.
#
# Two forms were tried and rejected before this one, each measured directly against a real cgroup, not
# assumed:
#   1. `head -c 512M /dev/zero | tail -c 1 >/dev/null`: `tail -c` streams its input rather than holding
#      it, so no single process ever actually holds the 512 MB in memory at once.
#   2. A Python one-liner building `bytearray(512 * 1024 * 1024)`: measured against a real 256M scope on
#      this batch's own dev machine, this allocation finished successfully, `memory.current` never rose
#      above ~25 MB, and no OOM kill occurred. The cause (review 206): a large zeroed allocation is lazy
#      — the allocator maps zero pages, but a page is only committed to real memory once something
#      writes to it. `bytearray(n)` never writes to the bytes it allocates, so almost nothing is ever
#      committed; it does not hold real memory, for a different reason than `tail -c` does not.
# What does hold real memory, confirmed the same way (measured, not assumed): `os.urandom(512 * 1024 *
# 1024)` holds it because it writes every one of those bytes, forcing every page it touches to be
# committed. Against the same 256M scope, this reliably drove `memory.current` well past the cap and
# produced a clean `oom_kill 1` with the process killed by `SIGKILL`, the scope's wrapping shell
# surviving to report it.
#
# `-p OOMPolicy=continue` is also needed, and was not obvious from a local, otherwise-identical run:
# this scope's *default* OOMPolicy (confirmed with `systemctl --user show -p OOMPolicy`) is `stop` —
# once systemd itself observes the kernel's OOM kill (via this same memory.events), it reacts by
# gracefully stopping the whole scope, SIGTERM first. That races our own wrapping shell's attempt to
# read memory.events and write it to MEM_EVENTS_FILE before the scope exits: CI's two first attempts at
# this (same code, both reruns of the same commit) show both outcomes — one run's shell won the race and
# reported a clean oom_kill 1, the other's lost it, killed by that reactive SIGTERM (`Terminated`, not
# `Killed`) before it could write anything, read back here as `unknown`. `OOMPolicy=continue` tells
# systemd not to react to the OOM kill at all, so the only thing that can still end the scope is the
# wrapping shell's own, deliberate exit once it has finished reading and writing the file — no more race.
if [ "$use_systemd_run" -eq 1 ]; then
    echo "self-test: memory proof — systemd-run --user --scope -p MemoryMax=256M -p MemorySwapMax=0 -p OOMPolicy=continue, holding ~512 MB"
    mem_hog='
cg="/sys/fs/cgroup$(cut -d: -f3 /proc/self/cgroup)"
python3 -c "import os, time; b = os.urandom(512 * 1024 * 1024); time.sleep(5)" &
hog_pid=$!
wait "$hog_pid" 2>/dev/null
grep -E "^oom_kill " "$cg/memory.events" >"$MEM_EVENTS_FILE" 2>/dev/null || echo "oom_kill unknown" >"$MEM_EVENTS_FILE"
'
    systemd-run --user --scope -p MemoryMax=256M -p MemorySwapMax=0 -p OOMPolicy=continue \
        --setenv="MEM_EVENTS_FILE=$mem_events_file" -- bash -c "$mem_hog"

    oom_kill_count=$(awk '{print $2}' "$mem_events_file" 2>/dev/null)
    echo "self-test: checked memory.events' oom_kill after holding ~512 MB under a 256M cap: ${oom_kill_count:-unknown}"
    if [ -n "$oom_kill_count" ] && [ "$oom_kill_count" -gt 0 ] 2>/dev/null; then
        echo "OK: the containment's memory bound killed the process that exceeded it (memory proof)"
    else
        echo "FAILED: memory.events' oom_kill was not above 0 — the memory bound was not enforced" >&2
        fail=1
    fi
else
    echo "self-test: memory proof skipped — no real cgroup scope was confirmed above, and the ulimit" \
        "fallback has no memory.events to read"
fi

if [ "$fail" -ne 0 ]; then
    exit 1
fi
