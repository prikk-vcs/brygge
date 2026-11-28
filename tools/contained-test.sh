#!/usr/bin/env bash
# P-5 (RFC 022 incident 094, HANDOFF.md §7a): every local test run uses this instead of bare
# `cargo test`. A test that spawns more than the limit's share of processes, or allocates past its
# share of memory, fails *inside* the limit here — instead of taking the machine down, as the
# pre-incident watchdog's fork bomb did.
#
# It tries `systemd-run --user --scope` first (a real cgroup bound: at most 256 tasks, 8 GiB memory,
# for the whole contained run, processes and threads together); where that is unavailable, a subshell
# under `ulimit -u 512 -v 8000000` (processes, and virtual memory in KiB). It states which it used, on
# its first line of output, and never runs unlimited either way.
#
# P-6 (RFC 022 incident 094): lists the system temporary directory's brygge-named entries before and
# after the run, and fails if any were added — the test suite must remove what it creates there.
#
# Usage: tools/contained-test.sh <cargo test arguments...>
# Example: tools/contained-test.sh --workspace --locked --no-fail-fast
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

tmp_dir=${TMPDIR:-/tmp}

# P-6: a snapshot of what brygge's own test suite might have left behind in the system temp directory,
# taken before this run so a later diff names only what this run itself added. Not `brygge-tmp-*`
# (a live artifact write in progress elsewhere on this machine would be a false positive) but the
# specific forms brygge's own tests and tools are known to name: `brygge-cli-*`, `brygge-atomic-*`,
# `brygge-bench-*`, and any bare `brygge*` this scan has not anticipated — the point of the check is to
# notice a new one, named or not.
snapshot() {
    find "$tmp_dir" -maxdepth 1 -name 'brygge*' 2>/dev/null | sort
}

before=$(snapshot)

status=0
if command -v systemd-run >/dev/null 2>&1 &&
    systemd-run --user --scope -p TasksMax=8 -p MemoryMax=64M -- true >/dev/null 2>&1; then
    echo "contained-test: systemd-run --user --scope (TasksMax=256, MemoryMax=8G)"
    systemd-run --user --scope -p TasksMax=256 -p MemoryMax=8G -- \
        cargo test "$@" || status=$?
else
    echo "contained-test: ulimit -u 512 -v 8000000 (systemd-run unavailable or not usable here)"
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
