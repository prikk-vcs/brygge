#!/usr/bin/env bash
# P-5 (RFC 022 incident 094, HANDOFF.md §7a): every local test run uses this instead of bare
# `cargo test`. A test that spawns more than the limit's share of processes, or allocates past its
# share of memory, fails *inside* the limit here — instead of taking the machine down, as the
# pre-incident watchdog's fork bomb did.
#
# review 208 R-7: the containment itself (the `systemd-run` scope, its probe-and-read-back, the `ulimit`
# fallback, and the CPU cap added that round) now lives in `tools/contained.sh -- <command...>`, shared
# with any other heavy local command. This script is a thin wrapper around it: `tools/contained.sh --
# cargo test "$@"`, plus the P-6 temp-leak check below, which is specific to the test suite and does not
# belong in the generic script.
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
# taken before this run so a later diff names only what this run itself added. The code matches every
# `brygge*` glob — not just the specific forms brygge's own tests and tools are known to name
# (`brygge-cli-*`, `brygge-atomic-*`, `brygge-bench-*`) — because the point of the check is to notice a
# new one too, named or not.
snapshot() {
    find "$tmp_dir" -maxdepth 1 -name 'brygge*' 2>/dev/null | sort
}

before=$(snapshot)

status=0
tools/contained.sh -- cargo test "$@" || status=$?

after=$(snapshot)
leaked=$(comm -13 <(echo "$before") <(echo "$after"))
if [ -n "$leaked" ]; then
    echo "FAILED (P-6, incident 094): the test suite left temporary files behind in $tmp_dir:" >&2
    echo "$leaked" >&2
    exit 1
fi

exit "$status"
