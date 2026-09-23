#!/usr/bin/env bash
# Publish the six brygge crates to crates.io, in dependency order, safely re-runnable (RFC 012 D-5).
#
# For each crate: skip it if <X.Y.Z> is already in the crates.io index; otherwise `cargo publish -p <crate>
# --locked`. On crates.io's rate limit (HTTP 429) wait for the time it states, at most 15 minutes, and retry
# once. Logs exactly which crates were published and which were skipped. A failure stops at once: what was
# published stays published (crates.io cannot roll back), and re-running finishes the rest.
#
# The credential is `CARGO_REGISTRY_TOKEN` in the environment (the release workflow obtains a short-lived
# one by trusted publishing); this script never reads or prints it.
#
# It publishes from the repository it is run in (the current directory's), wherever the script itself lives.
#
# Usage: tools/publish-crates.sh [--dry-run] <X.Y.Z>
#   --dry-run  decide and log, publish nothing (used by the tool tests and for a manual rehearsal)
set -euo pipefail

# Dependency order: the IR core first, the four decoders, then the CLI that depends on them all.
crates=(brygge-ir brygge-decode-cvs brygge-decode-git brygge-decode-hg brygge-decode-svn brygge)
max_wait=900

dry_run=0
if [ "${1:-}" = "--dry-run" ]; then
    dry_run=1
    shift
fi
if [ $# -ne 1 ] || ! [[ $1 =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "usage: tools/publish-crates.sh [--dry-run] <X.Y.Z>" >&2
    exit 2
fi
version=$1

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
cd "$(git rev-parse --show-toplevel)" || {
    echo "publish-crates: run it inside the brygge repository" >&2
    exit 2
}

published=()
skipped=()
planned=()

# The command that publishes one crate. BRYGGE_TEST_PUBLISH_CMD stands in for it in the tool tests
# (called as `<cmd> <crate>`); nothing else sets it.
publish_one() {
    if [ -n "${BRYGGE_TEST_PUBLISH_CMD:-}" ]; then
        "$BRYGGE_TEST_PUBLISH_CMD" "$1"
    else
        cargo publish -p "$1" --locked
    fi
}

# Seconds to wait for a rate limit, from crates.io's "try again after <RFC 1123 date>"; empty when the
# output states no time.
wait_seconds() {
    local stamp now until
    stamp=$(printf '%s' "$1" | grep -oE 'after [A-Za-z]{3}, [0-9]{1,2} [A-Za-z]{3} [0-9]{4} [0-9]{2}:[0-9]{2}:[0-9]{2} GMT' | head -n 1 | sed 's/^after //') || true
    [ -n "$stamp" ] || return 0
    until=$(date -u -d "$stamp" +%s) || return 0
    now=$(date -u +%s)
    echo $((until - now + 1))
}

for crate in "${crates[@]}"; do
    rc=0
    "$here/crate-published.sh" "$crate" "$version" || rc=$?
    case $rc in
        0)
            echo "skipped   $crate $version (already on crates.io)"
            skipped+=("$crate")
            continue
            ;;
        1) ;;
        *)
            echo "FAILED: could not tell whether $crate $version is on crates.io" >&2
            exit 1
            ;;
    esac

    if [ "$dry_run" -eq 1 ]; then
        echo "would publish $crate $version (dry run)"
        planned+=("$crate")
        continue
    fi

    echo "publishing $crate $version"
    log=$(mktemp)
    ok=0
    publish_one "$crate" >"$log" 2>&1 && ok=1
    cat "$log"
    if [ "$ok" -eq 0 ] && grep -qE 'HTTP status client error \(429|429 Too Many Requests|status 429' "$log"; then
        secs=$(wait_seconds "$(cat "$log")")
        if [ -z "$secs" ]; then
            echo "FAILED: crates.io rate-limited $crate but stated no time to wait for" >&2
            rm -f "$log"
            exit 1
        fi
        if [ "$secs" -gt "$max_wait" ]; then
            echo "FAILED: crates.io asks to wait ${secs}s for $crate, more than the ${max_wait}s bound; re-run later" >&2
            rm -f "$log"
            exit 1
        fi
        [ "$secs" -gt 0 ] || secs=1
        echo "rate limited: waiting ${secs}s, then retrying $crate once"
        sleep "$secs"
        publish_one "$crate" && ok=1
    fi
    rm -f "$log"
    if [ "$ok" -eq 0 ]; then
        echo "FAILED: $crate $version was not published" >&2
        exit 1
    fi
    published+=("$crate")
done

summary="published: ${published[*]:-none}; skipped (already on crates.io): ${skipped[*]:-none}"
[ "$dry_run" -eq 0 ] || summary="$summary; would publish (dry run): ${planned[*]:-none}"
echo "$summary"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    {
        echo "### crates.io publication for $version"
        echo
        echo "- **published:** ${published[*]:-none}"
        echo "- **skipped (already on crates.io):** ${skipped[*]:-none}"
    } >>"$GITHUB_STEP_SUMMARY"
fi
