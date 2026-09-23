#!/usr/bin/env bash
# Check that every relative markdown link in every *.md file under the repository (excluding
# .git-exclude/ and target/) resolves to an existing file (RFC 009 project-hygiene handoff, CR-14).
# Usage: tools/check-links.sh
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

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
        fi
    done < <(printf '%s\n' "$stripped" | grep -oP '(?<=\]\()[^)]+(?=\))' || true)
done < <(find . -name '*.md' -not -path './.git-exclude/*' -not -path './target/*' -not -path './.git/*' -print0)

if [ "$fail" -eq 0 ]; then
    echo "OK: $count relative link(s) checked across all *.md files, all resolve."
else
    echo "FAILED: one or more broken links found (see BROKEN lines above)."
    exit 1
fi
