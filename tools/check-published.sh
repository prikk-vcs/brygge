#!/usr/bin/env bash
# Byte-identity of the published crates with the tagged source (RFC 012 D-6). For each of the six crates
# this downloads the `.crate` that crates.io serves for <X.Y.Z>, then checks that
#   - `.cargo_vcs_info.json` names the tagged commit and the crate's path in the repository, and
#   - every file in the package is byte-identical to `git show <tag>:crates/<crate>/<path>`.
# The files cargo generates are not compared: `Cargo.toml` (rewritten on packaging) and `Cargo.lock`
# (generated for a package with a binary). `Cargo.toml.orig`, the original manifest, is compared with the
# source's `Cargo.toml`.
#
# Any difference, missing file or unexpected path fails with a message naming it. crates.io cannot be
# rolled back, but a mismatch must never pass silently. Run by the release workflow after publishing, and
# by hand as the manual fallback.
#
# It checks the repository it is run in (the current directory's), wherever the script itself lives.
#
# Usage: tools/check-published.sh <X.Y.Z> [<tag>]     (the tag defaults to the version)
# Needs: git (the tag present), curl, tar, jq, cmp.
set -euo pipefail

crates=(brygge-ir brygge-decode-cvs brygge-decode-git brygge-decode-hg brygge-decode-svn brygge)

if [ $# -lt 1 ] || [ $# -gt 2 ] || ! [[ $1 =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "usage: tools/check-published.sh <X.Y.Z> [<tag>]" >&2
    exit 2
fi
version=$1
tag=${2:-$version}

cd "$(git rev-parse --show-toplevel)" || {
    echo "check-published: run it inside the brygge repository" >&2
    exit 2
}

commit=$(git rev-parse --verify --quiet "refs/tags/$tag^{commit}") || {
    echo "check-published: the tag '$tag' is not in this repository (fetch it: git fetch --tags)" >&2
    exit 1
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
download=${BRYGGE_TEST_CRATES_DOWNLOAD:-https://crates.io/api/v1/crates}

failures=0
fail() {
    echo "MISMATCH: $*" >&2
    failures=$((failures + 1))
}

for crate in "${crates[@]}"; do
    pkg="$crate-$version"
    file="$work/$pkg.crate"
    curl --silent --show-error --fail --location --retry 3 --retry-all-errors \
        --user-agent "brygge-release-tools (https://github.com/prikk-vcs/brygge)" \
        --output "$file" "$download/$crate/$version/download" || {
        echo "check-published: cannot download $crate $version" >&2
        exit 1
    }

    # Refuse an archive that could write outside its own directory before extracting anything.
    if tar -tzf "$file" | grep -qvE "^$pkg/[^/].*$" || tar -tzf "$file" | grep -qE '(^|/)\.\.(/|$)'; then
        fail "$pkg.crate has an entry outside $pkg/ or with '..' in it"
        continue
    fi
    tar -xzf "$file" -C "$work"
    dir="$work/$pkg"

    vcs="$dir/.cargo_vcs_info.json"
    if [ ! -f "$vcs" ]; then
        fail "$pkg has no .cargo_vcs_info.json"
    else
        got=$(jq -r '.git.sha1 // empty' "$vcs")
        path_in_vcs=$(jq -r '.path_in_vcs // empty' "$vcs")
        [ "$got" = "$commit" ] || fail "$pkg was packaged from commit '$got', not the tagged $commit"
        [ "$path_in_vcs" = "crates/$crate" ] || fail "$pkg names path '$path_in_vcs', expected crates/$crate"
    fi

    checked=0
    while IFS= read -r -d '' path; do
        rel=${path#"$dir"/}
        case $rel in
            .cargo_vcs_info.json | Cargo.toml | Cargo.lock) continue ;;
        esac
        # `Cargo.toml.orig` is the crate's original manifest: the source's own `Cargo.toml`.
        src=$rel
        [ "$rel" != Cargo.toml.orig ] || src=Cargo.toml
        if ! git cat-file -e "$tag:crates/$crate/$src" 2>/dev/null; then
            fail "$pkg contains $rel, which is not in the tagged source crates/$crate/$src"
        elif ! git cat-file blob "$tag:crates/$crate/$src" | cmp -s - "$path"; then
            fail "$pkg/$rel differs from the tagged source crates/$crate/$src"
        fi
        checked=$((checked + 1))
    done < <(find "$dir" -type f -print0)
    echo "$pkg: $checked files compared with $tag ($commit), commit and path recorded in .cargo_vcs_info.json checked"
done

if [ "$failures" -ne 0 ]; then
    echo "FAILED: $failures mismatch(es) between the published crates and the tagged source" >&2
    exit 1
fi
echo "OK: all ${#crates[@]} published crates are byte-identical to the source at $tag"
