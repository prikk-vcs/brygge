#!/usr/bin/env bash
# Build the book with mdBook and fail on any warning (RFC 012 D-11). `mdbook build` exits 0 even when it warns
# (an unclosed HTML tag, a missing file, a broken link), so a warning is turned into a failure here. Both
# `docs.yml` (the deploy) and `ci.yml` (the check on every push and pull request) call this script.
#
# The output goes to <book-dir>/book (for this repository, `docs/book`, which is git-ignored).
#
# Usage: tools/build-book.sh [<book-dir>]      (default: docs; an argument is for the script's own tests)
set -euo pipefail

if [ $# -gt 1 ]; then
    echo "usage: tools/build-book.sh [<book-dir>]" >&2
    exit 2
fi
if [ $# -eq 1 ]; then
    book=$1
else
    cd "$(dirname "${BASH_SOURCE[0]}")/.."
    book=docs
fi

log=$(mktemp)
trap 'rm -f "$log"' EXIT

if ! NO_COLOR=1 mdbook build "$book" >"$log" 2>&1; then
    cat "$log" >&2
    echo "FAILED: mdbook build did not finish" >&2
    exit 1
fi
cat "$log"

# mdBook logs `INFO`, `WARN` and `ERROR` lines, each led by its level.
if grep -qE '^ *(WARN|ERROR)( |$)' "$log"; then
    echo "FAILED: mdbook build printed warnings or errors (above); the book must build clean" >&2
    exit 1
fi
echo "OK: the book built with no warnings"
