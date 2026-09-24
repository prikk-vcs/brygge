#!/usr/bin/env bash
# Check that every relative markdown link in every *.md file under the repository (excluding
# .git-exclude/ and target/) resolves to an existing file (RFC 009 project-hygiene handoff, CR-14).
#
# A second rule (RFC 012 D-11): a relative link in a page of the book, `docs/src/**/*.md`, must resolve to a
# file inside `docs/src/`. A link that leaves it resolves in the repository but is a 404 on the published site
# (https://prikk-vcs.github.io/brygge/), which contains only the book. Use an absolute
# https://github.com/prikk-vcs/brygge/blob/main/... URL for such a target.
#
# Usage: tools/check-links.sh [<repository-root>]     (the root defaults to this script's repository;
#                                                      an argument is for the script's own tests)
set -euo pipefail

if [ $# -gt 1 ]; then
    echo "usage: tools/check-links.sh [<repository-root>]" >&2
    exit 2
fi
if [ $# -eq 1 ]; then
    cd "$1"
else
    cd "$(dirname "${BASH_SOURCE[0]}")/.."
fi
root=$PWD

fail=0
count=0

while IFS= read -r -d '' file; do
    dir=$(dirname "$file")
    # Strip fenced code blocks (``` ... ```) first: a link shown as *illustrative markdown syntax*
    # inside a code fence (e.g. a policy doc's example) is not a real reference and must not be
    # followed, matching how real markdown renderers and link checkers treat code fences.
    stripped=$(awk '
        /^```/ { infence = !infence; next }
        !infence { print }
    ' "$file")
    # Extract markdown link targets: [text](target).
    while IFS= read -r target; do
        [ -z "$target" ] && continue
        # Skip absolute URLs, mailto, and in-page anchors.
        case "$target" in
            http://*|https://*|mailto:*|\#*) continue ;;
        esac
        # Strip a trailing in-page anchor (path.md#section -> path.md).
        path="${target%%#*}"
        [ -z "$path" ] && continue
        count=$((count + 1))
        resolved="$dir/$path"
        if [ ! -e "$resolved" ]; then
            echo "BROKEN: $file -> $target (resolved: $resolved)"
            fail=1
        else
            case $file in
                ./docs/src/*)
                    # Normalise (`..` and `.`) and require the target to be inside docs/src/.
                    normal=$(realpath -m -- "$resolved")
                    case $normal in
                        "$root"/docs/src/*) ;;
                        *)
                            echo "OUTSIDE THE BOOK: $file -> $target (resolves to ${normal#"$root"/}, outside docs/src/, so it is a 404 on the site; use an absolute https://github.com/prikk-vcs/brygge/blob/main/... URL)"
                            fail=1
                            ;;
                    esac
                    ;;
            esac
        fi
    done < <(printf '%s\n' "$stripped" | grep -oP '(?<=\]\()[^)]+(?=\))' || true)
done < <(find . -name '*.md' -not -path './.git-exclude/*' -not -path './target/*' -not -path './.git/*' -print0)

if [ "$fail" -eq 0 ]; then
    echo "OK: $count relative link(s) checked across all *.md files, all resolve."
else
    echo "FAILED: one or more broken or outside-the-book links found (see the lines above)."
    exit 1
fi
