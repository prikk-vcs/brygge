#!/usr/bin/env bash
# Tests for the documentation tools (RFC 012 D-11), in scratch trees.
#   tools/check-links.sh: a link that resolves inside the book passes; a link from a page of the book to a file
#     outside `docs/src/` is refused, however it is written; a broken link is still refused; and the
#     outside-the-book rule applies only to the book.
#   tools/build-book.sh: a book that builds clean passes; a book that makes mdBook warn (an unclosed HTML tag)
#     fails, though `mdbook build` itself exits 0.
#
# Usage: tools/test-docs-tools.sh    (needs the pinned mdBook: tools/install-mdbook.sh)
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
check=$PWD/tools/check-links.sh

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

passed=0
n=0
# new_tree: a fresh scratch repository with a README, an RFC and two pages of the book; `page <path> <text>`
# then writes one more page.
new_tree() {
    n=$((n + 1))
    tree=$scratch/t$n
    mkdir -p "$tree/docs/src/sub" "$tree/rfcs"
    printf 'x\n' >"$tree/README.md"
    printf 'x\n' >"$tree/rfcs/r.md"
    printf 'x\n' >"$tree/docs/src/b.md"
    printf 'x\n' >"$tree/docs/src/sub/c.md"
}
page() { printf '%s\n' "$2" >"$tree/$1"; }

expect_pass() {
    local what=$1
    if ! "$check" "$tree" >"$scratch/out" 2>&1; then
        cat "$scratch/out" >&2
        echo "FAIL $what: expected a pass" >&2
        exit 1
    fi
    passed=$((passed + 1))
    echo "ok   $what"
}
expect_fail() {
    local what=$1 needle=$2
    if "$check" "$tree" >"$scratch/out" 2>&1; then
        cat "$scratch/out" >&2
        echo "FAIL $what: expected a failure" >&2
        exit 1
    fi
    grep -qF -- "$needle" "$scratch/out" || {
        cat "$scratch/out" >&2
        echo "FAIL $what: the failure did not say '$needle'" >&2
        exit 1
    }
    passed=$((passed + 1))
    echo "ok   $what"
}

new_tree
page docs/src/a.md 'See [b](b.md), [c](sub/c.md#anchor) and [up](sub/../b.md).'
page docs/src/sub/d.md 'See [a page above](../b.md) and [a sibling](c.md).'
expect_pass "links that stay inside docs/src pass"

new_tree
page docs/src/a.md 'See [readme](../../README.md).'
expect_fail "a page of the book linking to the repository README is refused" "OUTSIDE THE BOOK: ./docs/src/a.md"

new_tree
page docs/src/sub/d.md 'See [rfc](../../../rfcs/r.md).'
expect_fail "a deeper page linking to an RFC outside the book is refused" "resolves to rfcs/r.md"

new_tree
page docs/src/a.md 'See [sneaky](sub/../../../README.md).'
expect_fail "an outside link disguised with sub/../ is refused" "OUTSIDE THE BOOK"

new_tree
page docs/src/a.md 'See [readme](../../README.md#top).'
expect_fail "an outside link with an anchor is refused" "OUTSIDE THE BOOK"

new_tree
page docs/src/a.md 'See [gone](missing.md).'
expect_fail "a broken link is still refused" "BROKEN: ./docs/src/a.md"

new_tree
page README.md 'See [the book](docs/src/b.md) and [an rfc](rfcs/r.md).'
page rfcs/r.md 'See [readme](../README.md).'
expect_pass "pages outside the book may link anywhere in the repository"

new_tree
fence='```'
printf 'An example:\n\n%s\n[outside](../../README.md)\n%s\n' "$fence" "$fence" >"$tree/docs/src/a.md"
expect_pass "a link shown inside a code fence is not followed"

new_tree
page docs/src/a.md 'See [site](https://github.com/prikk-vcs/brygge/blob/main/README.md).'
expect_pass "an absolute URL to the repository is the way to link outside the book"

# ---- tools/build-book.sh ---------------------------------------------------------------------------------
build=$PWD/tools/build-book.sh
new_book() {
    n=$((n + 1))
    book=$scratch/book$n
    mkdir -p "$book/src"
    printf '[book]\ntitle = "t"\nsrc = "src"\n' >"$book/book.toml"
    printf '# Summary\n\n[One](one.md)\n' >"$book/src/SUMMARY.md"
}
new_book
# shellcheck disable=SC2016  # the backticks are markdown, not a command substitution
printf '# One\n\nA `<form>` in code, and a table:\n\n| a | b |\n|---|---|\n| 1 | 2 |\n' >"$book/src/one.md"
if ! "$build" "$book" >"$scratch/out" 2>&1; then
    cat "$scratch/out" >&2
    echo "FAIL build-book: a clean book should pass" >&2
    exit 1
fi
grep -q "no warnings" "$scratch/out"
passed=$((passed + 1))
echo "ok   build-book: a clean book passes"

new_book
printf '# One\n\nThe artifact was made from a <form>; verify against it.\n' >"$book/src/one.md"
# mdBook itself exits 0 here; only the script turns the warning into a failure.
mdbook build "$book" >/dev/null 2>&1
if "$build" "$book" >"$scratch/out" 2>&1; then
    cat "$scratch/out" >&2
    echo "FAIL build-book: a book that makes mdBook warn should fail" >&2
    exit 1
fi
grep -q "unclosed HTML tag" "$scratch/out" && grep -q "must build clean" "$scratch/out"
passed=$((passed + 1))
echo "ok   build-book: an unclosed HTML tag (mdBook exits 0, warns) fails the script"

echo "all $passed documentation-tool tests passed"
