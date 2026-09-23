#!/usr/bin/env bash
# Tests for the release tools (RFC 012 §4): release-notes.sh, crate-published.sh, publish-crates.sh,
# check-release-tag.sh and check-published.sh. Positive cases run against the real 0.1.0 (published, so
# they need the network and the `0.1.0` tag); negative cases run in scratch repositories and against a
# local stand-in for crates.io, so they can fail without touching anything real. CI runs this file.
#
# Usage: tools/test-release-tools.sh
# Needs: bash, git, cargo, jq, curl, tar, python3.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
repo=$PWD

scratch=$(mktemp -d)
server_pid=""
cleanup() {
    [ -z "$server_pid" ] || kill "$server_pid" 2>/dev/null || true
    rm -rf "$scratch"
}
trap cleanup EXIT

passed=0
pass() {
    passed=$((passed + 1))
    echo "ok   $*"
}
die() {
    echo "FAIL $*" >&2
    exit 1
}

# expect_ok <what> <command...>: the command succeeds.
expect_ok() {
    local what=$1
    shift
    "$@" >"$scratch/out" 2>&1 || {
        cat "$scratch/out" >&2
        die "$what: expected success"
    }
    pass "$what"
}

# expect_fail <what> <needle> <command...>: the command fails, and its output contains <needle>.
expect_fail() {
    local what=$1 needle=$2
    shift 2
    if "$@" >"$scratch/out" 2>&1; then
        cat "$scratch/out" >&2
        die "$what: expected a failure"
    fi
    grep -qF -- "$needle" "$scratch/out" || {
        cat "$scratch/out" >&2
        die "$what: the failure did not mention '$needle'"
    }
    pass "$what"
}

# A local stand-in for crates.io: a static HTTP server over $scratch/www.
mkdir -p "$scratch/www"
start_server() {
    (cd "$scratch/www" && exec python3 -u -m http.server 0 --bind 127.0.0.1) >"$scratch/server.log" 2>&1 &
    server_pid=$!

    for _ in $(seq 1 50); do
        if grep -q 'port [0-9]' "$scratch/server.log" 2>/dev/null; then
            port=$(grep -o 'port [0-9]*' "$scratch/server.log" | head -n 1 | cut -d' ' -f2)
            return 0
        fi
        sleep 0.1
    done
    die "the local test server did not start"
}
start_server
export BRYGGE_TEST_CRATES_INDEX="http://127.0.0.1:$port"
export BRYGGE_TEST_CRATES_DOWNLOAD="http://127.0.0.1:$port/api"

# ---- tools/release-notes.sh -------------------------------------------------------------------------
notes=$(tools/release-notes.sh 0.1.0)
case $notes in
    "**The first release.**"*) pass "release-notes 0.1.0 begins with the first-release line" ;;
    *) die "release-notes 0.1.0 does not begin with '**The first release.**'" ;;
esac
case $notes in
    *"The detailed changes"*) die "release-notes 0.1.0 contains 'The detailed changes'" ;;
    *) pass "release-notes 0.1.0 stops before 'The detailed changes'" ;;
esac
expect_fail "release-notes: a missing section fails" "found 0" tools/release-notes.sh 9.9.9
expect_fail "release-notes: a non-version is a usage error" "not a version" tools/release-notes.sh Unreleased

# A scratch CHANGELOG, to test the shapes the real one does not have (a section without a divider, an
# empty section, a duplicate heading, and 0.1.1 next to 0.1.10).
mkdir -p "$scratch/cl"
git init -q "$scratch/cl"
cat >"$scratch/cl/CHANGELOG.md" <<'CL'
# Changelog

## [Unreleased]

## [0.3.0] — 2026-10-01

Notes for 0.3.0.

- one
- two

## [0.2.0] — 2026-09-30

Notes for 0.2.0.

The detailed changes follow.

### an entry

## [0.1.10] — 2026-09-29

ten

## [0.1.1] — 2026-09-28

one

## [0.0.9] — 2026-09-27

## [0.0.8] — 2026-09-26

first

## [0.0.8] — 2026-09-25

again
CL
[ "$(cd "$scratch/cl" && "$repo/tools/release-notes.sh" 0.3.0)" = "$(printf 'Notes for 0.3.0.\n\n- one\n- two')" ] || die "release-notes: a section without a divider runs to the next heading, trimmed"
pass "release-notes: a section without a divider runs to the next heading, trimmed"
[ "$(cd "$scratch/cl" && "$repo/tools/release-notes.sh" 0.2.0)" = "Notes for 0.2.0." ] || die "release-notes: a divider ends the notes"
pass "release-notes: a divider ends the notes"
[ "$(cd "$scratch/cl" && "$repo/tools/release-notes.sh" 0.1.1)" = "one" ] || die "release-notes: 0.1.1 is not confused with 0.1.10"
pass "release-notes: 0.1.1 is not confused with 0.1.10"
expect_fail "release-notes: an empty section fails" "has no text" bash -c "cd '$scratch/cl' && '$repo/tools/release-notes.sh' 0.0.9"
expect_fail "release-notes: a duplicate heading fails" "found 2" bash -c "cd '$scratch/cl' && '$repo/tools/release-notes.sh' 0.0.8"

# ---- tools/crate-published.sh -----------------------------------------------------------------------
# Against the real crates.io index (the default), not the stand-in.
real_index() { env -u BRYGGE_TEST_CRATES_INDEX "$@"; }
expect_ok "crate-published: brygge 0.1.0 is on crates.io" real_index tools/crate-published.sh brygge 0.1.0
expect_ok "crate-published: brygge-ir 0.1.0 is on crates.io" real_index tools/crate-published.sh brygge-ir 0.1.0
rc=0
real_index tools/crate-published.sh brygge 99.0.0 || rc=$?
[ "$rc" -eq 1 ] || die "crate-published: an unpublished version should exit 1, got $rc"
pass "crate-published: an unpublished version exits 1"
rc=0
tools/crate-published.sh brygge-ir 1.2.3 || rc=$?
[ "$rc" -eq 1 ] || die "crate-published: a crate absent from the index should exit 1, got $rc"
pass "crate-published: a crate absent from the (local) index exits 1"
rc=0
BRYGGE_TEST_CRATES_INDEX="http://127.0.0.1:1" tools/crate-published.sh brygge-ir 1.2.3 2>/dev/null || rc=$?
[ "$rc" -eq 2 ] || die "crate-published: an unreachable index should exit 2, got $rc"
pass "crate-published: an unreachable index exits 2 (it never guesses 'not published')"

# ---- tools/publish-crates.sh ------------------------------------------------------------------------
dry=$(env -u BRYGGE_TEST_CRATES_INDEX tools/publish-crates.sh --dry-run 0.1.0)
[ "$(printf '%s\n' "$dry" | grep -c '^skipped ')" -eq 6 ] || die "publish-crates: a dispatch for 0.1.0 should skip all six crates"
pass "publish-crates: 0.1.0 (already published) skips all six crates and publishes nothing"

# A fake `cargo publish`: logs the crate, and adds it to the local index unless told to fail.
cat >"$scratch/fake-publish.sh" <<'FAKE'
#!/usr/bin/env bash
set -euo pipefail
crate=$1
echo "$crate" >>"$FAKE_LOG"
mode_file="$FAKE_MODES/$crate"
mode=ok
if [ -f "$mode_file" ]; then
    mode=$(head -n 1 "$mode_file")
    # a mode is used once: the retry succeeds
    tail -n +2 "$mode_file" >"$mode_file.rest" && mv "$mode_file.rest" "$mode_file"
    [ -s "$mode_file" ] || rm -f "$mode_file"
fi
case $mode in
    ok)
        # record it in the local index, as crates.io would
        c=$crate
        d="$INDEX_DIR/${c:0:2}/${c:2:2}"
        mkdir -p "$d"
        echo "{\"name\":\"$c\",\"vers\":\"$FAKE_VERSION\"}" >>"$d/$c"
        ;;
    rate-limit-soon)
        echo "error: failed to publish to registry at https://crates.io" >&2
        echo "  the remote server responded with an error (status 429 Too Many Requests): You have published too many new crates in a short period of time. Please try again after $(LC_ALL=C date -u -d '+3 seconds' '+%a, %d %b %Y %H:%M:%S GMT') or email help@crates.io" >&2
        exit 101
        ;;
    rate-limit-late)
        echo "error: the remote server responded with an error (status 429 Too Many Requests): Please try again after $(LC_ALL=C date -u -d '+2 hours' '+%a, %d %b %Y %H:%M:%S GMT') or email help@crates.io" >&2
        exit 101
        ;;
    rate-limit-no-time)
        echo "error: the remote server responded with an error (status 429 Too Many Requests)" >&2
        exit 101
        ;;
    fail)
        echo "error: some other failure" >&2
        exit 101
        ;;
esac
FAKE
chmod +x "$scratch/fake-publish.sh"

publish_scenario() {
    export FAKE_LOG="$scratch/fake-log" FAKE_MODES="$scratch/fake-modes" INDEX_DIR="$scratch/www" FAKE_VERSION=9.9.9
    rm -rf "$scratch/www/"?? "$FAKE_MODES" "$FAKE_LOG"
    mkdir -p "$FAKE_MODES"
    : >"$FAKE_LOG"
    export BRYGGE_TEST_PUBLISH_CMD="$scratch/fake-publish.sh"
}
publish_end() { unset BRYGGE_TEST_PUBLISH_CMD FAKE_LOG FAKE_MODES INDEX_DIR FAKE_VERSION; }

publish_scenario
expect_ok "publish-crates: a clean run publishes all six" tools/publish-crates.sh 9.9.9
[ "$(paste -sd' ' "$FAKE_LOG")" = "brygge-ir brygge-decode-cvs brygge-decode-git brygge-decode-hg brygge-decode-svn brygge" ] || die "publish-crates: wrong order: $(cat "$FAKE_LOG")"
pass "publish-crates: the crates were published in dependency order"
: >"$FAKE_LOG"
expect_ok "publish-crates: a re-run after completion publishes nothing" tools/publish-crates.sh 9.9.9
[ ! -s "$FAKE_LOG" ] || die "publish-crates: a re-run published again: $(cat "$FAKE_LOG")"
grep -q 'published: none; skipped (already on crates.io): brygge-ir' "$scratch/out" || die "publish-crates: the re-run did not log its skips"
pass "publish-crates: the re-run logged six skips and no publication"

# A partial failure, then a re-run completes only the rest.
publish_scenario
echo fail >"$FAKE_MODES/brygge-decode-hg"
expect_fail "publish-crates: a failure stops the run" "brygge-decode-hg 9.9.9 was not published" tools/publish-crates.sh 9.9.9
[ "$(paste -sd' ' "$FAKE_LOG")" = "brygge-ir brygge-decode-cvs brygge-decode-git brygge-decode-hg" ] || die "publish-crates: it went on after a failure: $(cat "$FAKE_LOG")"
: >"$FAKE_LOG"
expect_ok "publish-crates: the re-run finishes the rest" tools/publish-crates.sh 9.9.9
[ "$(paste -sd' ' "$FAKE_LOG")" = "brygge-decode-hg brygge-decode-svn brygge" ] || die "publish-crates: the re-run did the wrong crates: $(cat "$FAKE_LOG")"
grep -q 'skipped   brygge-decode-git' "$scratch/out" || die "publish-crates: the re-run did not skip what was done"
pass "publish-crates: the re-run published only hg, svn and brygge, and skipped the three done"

# A rate limit with a near time: wait, retry once, succeed.
publish_scenario
echo rate-limit-soon >"$FAKE_MODES/brygge"
start=$(date +%s)
expect_ok "publish-crates: a 429 with a near time waits and retries once" tools/publish-crates.sh 9.9.9
waited=$(($(date +%s) - start))
[ "$waited" -ge 2 ] || die "publish-crates: it did not wait for the rate limit (${waited}s)"
[ "$(grep -c '^brygge$' "$FAKE_LOG")" -eq 2 ] || die "publish-crates: brygge should have been tried twice"
grep -q 'rate limited: waiting' "$scratch/out" || die "publish-crates: the wait was not logged"
pass "publish-crates: the wait (${waited}s) was logged and brygge was tried exactly twice"

# A rate limit past the bound, or without a time, or twice: fail without waiting or retrying more.
publish_scenario
echo rate-limit-late >"$FAKE_MODES/brygge-ir"
expect_fail "publish-crates: a wait past 15 minutes fails at once" "more than the 900s bound" tools/publish-crates.sh 9.9.9
publish_scenario
echo rate-limit-no-time >"$FAKE_MODES/brygge-ir"
expect_fail "publish-crates: a 429 without a stated time fails" "stated no time" tools/publish-crates.sh 9.9.9
publish_scenario
printf 'rate-limit-soon\nrate-limit-soon\n' >"$FAKE_MODES/brygge-ir"
expect_fail "publish-crates: a second 429 is not retried again" "brygge-ir 9.9.9 was not published" tools/publish-crates.sh 9.9.9
[ "$(grep -c '^brygge-ir$' "$FAKE_LOG")" -eq 2 ] || die "publish-crates: brygge-ir should have been tried exactly twice"
pass "publish-crates: brygge-ir was tried exactly twice"
publish_end
rm -rf "$scratch/www/"??

# ---- tools/check-release-tag.sh: scratch repositories ------------------------------------------------
# A minimal workspace with one crate. The real script (this repository's copy, not one inside the scratch
# repository) is run from inside the scratch repository, as the release workflow runs its own copy of the
# tools on the tagged tree.
new_repo() { # new_repo <name>: a clone `$scratch/<name>` of a bare origin, with main pushed
    local name=$1
    git init -q --bare --initial-branch=main "$scratch/$name-origin.git"
    git clone -q "$scratch/$name-origin.git" "$scratch/$name" 2>/dev/null
    (
        cd "$scratch/$name"
        git config user.email t@example.com
        git config user.name tester
        git config commit.gpgsign false
        git config tag.gpgsign false
        git checkout -q -b main
        mkdir -p crates/a/src
        printf '[workspace]\nmembers = ["crates/a"]\nresolver = "2"\n' >Cargo.toml
        printf '[package]\nname = "a"\nversion = "1.2.3"\nedition = "2021"\n' >crates/a/Cargo.toml
        : >crates/a/src/lib.rs
        printf '# Changelog\n\n## [Unreleased]\n\n## [1.2.3] — 2026-01-01\n\nNotes.\n' >CHANGELOG.md
        git add -A
        git commit -q -m "release 1.2.3"
        git push -q origin main 2>/dev/null
    )
}
tag_check() { (cd "$scratch/$1" && "$repo/tools/check-release-tag.sh" "$2"); }

new_repo good
(cd "$scratch/good" && git tag -a 1.2.3 -m "1.2.3")
expect_ok "check-release-tag: an annotated tag on main with a matching version and CHANGELOG passes" tag_check good 1.2.3

new_repo light
(cd "$scratch/light" && git tag 1.2.3)
expect_fail "check-release-tag: a lightweight tag fails" "is not annotated" tag_check light 1.2.3

new_repo missing
expect_fail "check-release-tag: a missing tag fails" "does not exist" tag_check missing 1.2.3

new_repo side
(
    cd "$scratch/side"
    git checkout -q -b side
    printf 'x\n' >extra.txt
    git add -A
    git commit -q -m "a commit that is not on main"
    git tag -a 1.2.3 -m "1.2.3"
)
expect_fail "check-release-tag: a tag not on main fails" "not reachable from origin/main" tag_check side 1.2.3

new_repo version
(
    cd "$scratch/version"
    sed -i 's/version = "1.2.3"/version = "1.2.4"/' crates/a/Cargo.toml
    git commit -q -am "bump to 1.2.4 but tag 1.2.3"
    git push -q origin main 2>/dev/null
    git tag -a 1.2.3 -m "1.2.3"
)
expect_fail "check-release-tag: a version mismatch fails" "a is 1.2.4" tag_check version 1.2.3

new_repo heading
(
    cd "$scratch/heading"
    sed -i 's/## \[1.2.3\]/## [1.2.2]/' CHANGELOG.md
    git commit -q -am "changelog without the heading"
    git push -q origin main 2>/dev/null
    git tag -a 1.2.3 -m "1.2.3"
)
expect_fail "check-release-tag: a missing CHANGELOG heading fails" "CHANGELOG.md is not ready for 1.2.3" tag_check heading 1.2.3

new_repo stale
(
    cd "$scratch/stale"
    git tag -a 1.2.3 -m "1.2.3"
    printf 'y\n' >later.txt
    git add -A
    git commit -q -m "a later commit"
    git push -q origin main 2>/dev/null
)
expect_fail "check-release-tag: a tree that is not the tagged commit fails" "is not the tagged commit" tag_check stale 1.2.3

# ---- tools/check-release-absent.sh: a fake `gh` -------------------------------------------------------
mkdir -p "$scratch/fakebin"
cat >"$scratch/fakebin/gh" <<'GH'
#!/usr/bin/env bash
# a fake `gh release view <tag>`: FAKE_GH says what it answers
case ${FAKE_GH:-exists} in
    exists) echo "title: 1.2.3"; exit 0 ;;
    absent) echo "release not found" >&2; exit 1 ;;
    broken) echo "HTTP 502: bad gateway" >&2; exit 1 ;;
esac
GH
chmod +x "$scratch/fakebin/gh"
absent() { FAKE_GH=$1 PATH="$scratch/fakebin:$PATH" tools/check-release-absent.sh 1.2.3; }
expect_ok "check-release-absent: no release yet passes" absent absent
expect_fail "check-release-absent: an existing release fails" "already exists" absent exists
rc=0
absent broken >/dev/null 2>&1 || rc=$?
[ "$rc" -eq 2 ] || die "check-release-absent: a gh failure should exit 2, got $rc"
pass "check-release-absent: a gh failure is 'unknown' (exit 2), never 'absent'"

# ---- tools/check-published.sh -------------------------------------------------------------------------
if git rev-parse --verify --quiet refs/tags/0.1.0 >/dev/null; then
    expect_ok "check-published: the published 0.1.0 crates are byte-identical to the tag" \
        env -u BRYGGE_TEST_CRATES_DOWNLOAD tools/check-published.sh 0.1.0

    # Serve the real 0.1.0 crates from the local stand-in, altered in the ways the check must catch.
    crates=(brygge-ir brygge-decode-cvs brygge-decode-git brygge-decode-hg brygge-decode-svn brygge)
    stage="$scratch/stage"
    mkdir -p "$stage"
    for c in "${crates[@]}"; do
        mkdir -p "$scratch/www/api/$c/0.1.0"
        curl --silent --show-error --fail --location --retry 3 --retry-all-errors \
            --user-agent "brygge-release-tools (https://github.com/prikk-vcs/brygge)" \
            --output "$scratch/www/api/$c/0.1.0/download" "https://crates.io/api/v1/crates/$c/0.1.0/download"
    done
    pristine="$scratch/pristine"
    cp -r "$scratch/www/api" "$pristine"

    alter() { # alter <crate> <edit-command...>: unpack the crate, run the edit inside it, repack
        local c=$1
        shift
        rm -rf "${stage:?}/$c-0.1.0"
        tar -xzf "$pristine/$c/0.1.0/download" -C "$stage"
        (cd "$stage/$c-0.1.0" && "$@")
        tar -czf "$scratch/www/api/$c/0.1.0/download" -C "$stage" "$c-0.1.0"
    }
    restore() { cp "$pristine/$1/0.1.0/download" "$scratch/www/api/$1/0.1.0/download"; }

    expect_ok "check-published: the local stand-in serving the untouched crates passes" tools/check-published.sh 0.1.0

    alter brygge-ir sh -c 'printf "// tampered\n" >>src/lib.rs'
    expect_fail "check-published: a changed source file fails" "brygge-ir-0.1.0/src/lib.rs differs" tools/check-published.sh 0.1.0
    restore brygge-ir

    alter brygge-decode-git sh -c 'printf "extra\n" >added.txt'
    expect_fail "check-published: an added file fails" "added.txt, which is not in the tagged source" tools/check-published.sh 0.1.0
    restore brygge-decode-git

    alter brygge-decode-hg sh -c 'printf "{\"git\":{\"sha1\":\"0000000000000000000000000000000000000000\"},\"path_in_vcs\":\"crates/brygge-decode-hg\"}" >.cargo_vcs_info.json'
    expect_fail "check-published: a crate packaged from another commit fails" "was packaged from commit" tools/check-published.sh 0.1.0
    restore brygge-decode-hg

    alter brygge sh -c 'printf "# tampered\n" >>Cargo.toml.orig'
    expect_fail "check-published: a changed original manifest fails" "brygge-0.1.0/Cargo.toml.orig differs" tools/check-published.sh 0.1.0
    restore brygge

    expect_fail "check-published: a tag that is not in the repository fails" "is not in this repository" tools/check-published.sh 0.1.0 no-such-tag
    expect_ok "check-published: the untouched stand-in passes again" tools/check-published.sh 0.1.0
else
    echo "skip check-published tests: the 0.1.0 tag is not fetched (git fetch --tags)"
fi

echo "all $passed release-tool checks passed"
