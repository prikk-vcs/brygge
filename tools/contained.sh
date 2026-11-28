#!/usr/bin/env bash
# The shared containment (RFC 022 incident 094, HANDOFF.md §7a), factored out of tools/contained-test.sh
# (review 208 R-7) so that any heavy local command — not only `cargo test` — runs inside the same bound.
# `tools/contained-test.sh` is now a thin wrapper around this script, adding only its own P-6 temp-leak
# check, which is specific to the test suite.
#
# It tries `systemd-run --user --scope` first (a real cgroup bound: at most 256 tasks, 8 GiB of memory
# with no swap, and a quarter of this machine's cores, for the whole contained run, processes and threads
# together); where that is unavailable, or where it is accepted but not actually enforced (review 205 R-2),
# a subshell under `ulimit -u 512 -v 8000000` instead (review 206: weaker, plainly — and, unlike
# `systemd-run`, unable to express a CPU cap at all). `-v 8000000` bounds each process's own address space
# to 8 GB, and `-u 512` bounds the user's process count — but not their sum: 512 processes at 8 GB each is
# far beyond this machine, and `ulimit` has no cgroup to put a ceiling on the total. `-u 512` also counts
# every process this user already has (a browser, a desktop session), so on a busy desktop the fallback
# may refuse to start at all — that fails closed, which is acceptable. It states which it used, and why, on
# its first line of output, and never runs unlimited either way.
#
# review 205 R-1: `MemoryMax` alone does not bound swap — above it, the kernel swaps a cgroup out instead
# of stopping it, and this machine's 62 GB zram swap device means a leaking command could still reach the
# incident's RAM-and-swap-both-above-90% state. `-p MemorySwapMax=0` makes `MemoryMax` a real ceiling:
# memory.swap.max is set to 0, so there is nothing to swap into.
#
# review 205 R-2: `systemd-run` accepting these properties only proves it understood the syntax — not that
# a cgroup controller enforcing them is actually delegated to this user's manager. The probe below
# therefore reads back its own scope's pids.max, memory.max, memory.swap.max and (review 208 R-7) cpu.max,
# and compares each to what it asked for; a mismatch on any of them means the limit would silently go
# unenforced, so the ulimit fallback is used instead, and the mismatch is named on the first line of
# output.
#
# review 208 R-7 (owner's note on CPU heat, review 209): every `cargo` build or test on this machine uses
# all of its cores at 100% by default, and CI already proves the same things on its own runners, so a
# local containment that bounds tasks and memory but not CPU still heats the machine for no real safety
# benefit. The cap is `-p CPUQuota=<N*100>%`, where N is a quarter of `nproc`, minimum 2 (8 on this
# machine's 32 cores). `CARGO_BUILD_JOBS` and `RUST_TEST_THREADS` are set to N for whatever runs inside the
# containment — under both the `systemd-run` path and the `ulimit` fallback — so the work fits inside the
# cap instead of spawning as many threads as there are cores and having them fight over a quarter of it.
# The `ulimit` fallback has no CPU cap of its own (`ulimit` cannot express one), but still sets the two job
# counts, and says plainly on its first line that it has no CPU cap.
#
# `--no-cpu-cap` skips the `-p CPUQuota=...` property (and its cpu.max read-back), keeping the task and
# memory limits. The measurement harness and the release-corpus timings use this, so that timings stay
# comparable between releases (HANDOFF §7).
#
# Usage: tools/contained.sh [--no-cpu-cap] -- <command...>
# Example: tools/contained.sh -- cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

no_cpu_cap=0
if [ "${1:-}" = "--no-cpu-cap" ]; then
    no_cpu_cap=1
    shift
fi
if [ "${1:-}" = "--" ]; then
    shift
fi
if [ "$#" -eq 0 ]; then
    echo "usage: tools/contained.sh [--no-cpu-cap] -- <command...>" >&2
    exit 2
fi

cores=$(nproc)
cpu_n=$((cores / 4))
if [ "$cpu_n" -lt 2 ]; then
    cpu_n=2
fi

# review 205 R-2: the probe's own cgroup readback has to leave the probe's scope as plain text on disk,
# the same way the self-test's own progress file already crosses the `systemd-run` boundary (review
# 204/205's shared lesson: shell state does not survive that boundary, a file does) — the scope, and the
# shell running inside it, are both gone by the time this script can next inspect anything.
probe_file=$(mktemp)
cleanup() { rm -f "$probe_file"; }
trap cleanup EXIT

# A plain command string, not a function, for the same reason check-contained-test-self-test.sh uses one:
# nothing here crosses the systemd-run boundary as shell state, only as this literal text.
probe_read_back='
cg="/sys/fs/cgroup$(cut -d: -f3 /proc/self/cgroup)"
{
    cat "$cg/pids.max" 2>/dev/null || echo unknown
    cat "$cg/memory.max" 2>/dev/null || echo unknown
    cat "$cg/memory.swap.max" 2>/dev/null || echo unknown
    cat "$cg/cpu.max" 2>/dev/null || echo unknown
} >"$PROBE_FILE"
'

use_systemd_run=0
enforcement_mismatch=""

probe_cpu_args=()
expected_cpu=""
if [ "$no_cpu_cap" -eq 0 ]; then
    probe_cpu_args=(-p "CPUQuota=$((cpu_n * 100))%")
    expected_cpu="$((cpu_n * 100000)) 100000"
fi

if command -v systemd-run >/dev/null 2>&1 &&
    systemd-run --user --scope -p TasksMax=8 -p MemoryMax=64M -p MemorySwapMax=0 "${probe_cpu_args[@]}" \
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
    if [ "$no_cpu_cap" -eq 0 ]; then
        [ "$probed_cpu" = "$expected_cpu" ] || reasons="${reasons}cpu.max=$probed_cpu, "
    fi

    if [ -z "$reasons" ]; then
        use_systemd_run=1
    else
        enforcement_mismatch="${reasons%, }"
    fi
fi

status=0
if [ "$use_systemd_run" -eq 1 ]; then
    if [ "$no_cpu_cap" -eq 1 ]; then
        echo "contained: systemd-run --user --scope (TasksMax=256, MemoryMax=8G, MemorySwapMax=0, no CPU cap (--no-cpu-cap) — read back and verified)"
        systemd-run --user --scope -p TasksMax=256 -p MemoryMax=8G -p MemorySwapMax=0 \
            --setenv="CARGO_BUILD_JOBS=$cpu_n" --setenv="RUST_TEST_THREADS=$cpu_n" -- "$@" || status=$?
    else
        echo "contained: systemd-run --user --scope (TasksMax=256, MemoryMax=8G, MemorySwapMax=0, CPUQuota=$((cpu_n * 100))% — read back and verified)"
        systemd-run --user --scope -p TasksMax=256 -p MemoryMax=8G -p MemorySwapMax=0 -p "CPUQuota=$((cpu_n * 100))%" \
            --setenv="CARGO_BUILD_JOBS=$cpu_n" --setenv="RUST_TEST_THREADS=$cpu_n" -- "$@" || status=$?
    fi
else
    if [ -n "$enforcement_mismatch" ]; then
        echo "contained: ulimit -u 512 -v 8000000, CARGO_BUILD_JOBS=$cpu_n RUST_TEST_THREADS=$cpu_n, no CPU cap (systemd-run's limits were not enforced: $enforcement_mismatch)"
    else
        echo "contained: ulimit -u 512 -v 8000000, CARGO_BUILD_JOBS=$cpu_n RUST_TEST_THREADS=$cpu_n, no CPU cap (systemd-run unavailable or not usable here)"
    fi
    (
        export CARGO_BUILD_JOBS="$cpu_n"
        export RUST_TEST_THREADS="$cpu_n"
        ulimit -u 512 -v 8000000
        "$@"
    ) || status=$?
fi

exit "$status"
