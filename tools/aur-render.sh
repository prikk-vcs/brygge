#!/usr/bin/env bash
# Render the two AUR PKGBUILDs (RFC 016 D-1) for one release: fills in the version and the checksums only
# — the rest of each PKGBUILD is the real, reviewed template at packaging/aur/<pkg>/PKGBUILD.in.
#
# It never computes a binary's checksum itself: it reads the ones the release already published
# (<assets-dir>/brygge-<target>.tar.gz.sha256), after the caller has verified each binary's attestation
# (release.yml's `aur` job does this with `gh attestation verify` before calling this script) — so the AUR
# never carries a checksum for a binary that was not attested. The one checksum this script does compute
# is the GitHub tag's own source archive, for the `brygge` (source) package.
#
# Refuses a missing asset or a malformed version; writes nothing in that case. Writes
# <out-dir>/brygge/PKGBUILD and <out-dir>/brygge-bin/PKGBUILD, then, only if `makepkg` is present (it runs
# inside the archlinux:base-devel container; a plain CI runner does not have it), the matching .SRCINFO
# via `makepkg --printsrcinfo` — skipped with a note otherwise, never a failure, so this script's own
# tests can run on an ordinary Linux CI runner.
#
# Exit status: 2 for a usage error (wrong argument count, a version not of the form X.Y.Z); 1 when an
# asset is missing or the tag archive cannot be downloaded or hashed. Never guesses.
#
# The AUR_PKGREL environment variable overrides pkgrel (default 1): RFC 016 D-3's packaging-only fix,
# dispatched with a `pkgrel` input, passes it through here.
#
# Usage: tools/aur-render.sh <X.Y.Z> <assets-dir> <out-dir>
# Needs: bash, curl, sha256sum, sed; optionally makepkg (for .SRCINFO).
set -euo pipefail

if [ $# -ne 3 ] || ! [[ $1 =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "usage: tools/aur-render.sh <X.Y.Z> <assets-dir> <out-dir>" >&2
    exit 2
fi
version=$1
assets=$2
out=$3
pkgrel=${AUR_PKGREL:-1}
if ! [[ $pkgrel =~ ^[0-9]+$ ]]; then
    echo "aur-render: AUR_PKGREL must be a positive integer, got '$pkgrel'" >&2
    exit 2
fi

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

need_asset() { # need_asset <path>: the file exists, or a typed failure names it
    if [ ! -f "$1" ]; then
        echo "aur-render: missing asset: $1" >&2
        exit 1
    fi
}

x86_64_tar="$assets/brygge-x86_64-unknown-linux-gnu.tar.gz"
x86_64_sum_file="$x86_64_tar.sha256"
aarch64_tar="$assets/brygge-aarch64-unknown-linux-gnu.tar.gz"
aarch64_sum_file="$aarch64_tar.sha256"
need_asset "$x86_64_tar"
need_asset "$x86_64_sum_file"
need_asset "$aarch64_tar"
need_asset "$aarch64_sum_file"

# Each .sha256 file holds one line: "<hash>  <filename>" (sha256sum's own format; see release.yml's
# `build` job, which writes these files for the exact binaries read here).
sum_of() { # sum_of <sha256-file> <expected-basename>
    local file=$1 name=$2 hash got_name
    hash=$(awk 'NR==1 { print $1; exit }' "$file")
    got_name=$(awk 'NR==1 { n = $2; sub(/^\*/, "", n); print n; exit }' "$file")
    got_name=$(basename "$got_name")
    if [ "$got_name" != "$name" ]; then
        echo "aur-render: $file names '$got_name', expected '$name'" >&2
        exit 1
    fi
    if ! [[ $hash =~ ^[0-9a-f]{64}$ ]]; then
        echo "aur-render: $file does not hold a 64-character hex sha256 ('$hash')" >&2
        exit 1
    fi
    printf '%s' "$hash"
}
sha256_x86_64=$(sum_of "$x86_64_sum_file" "$(basename "$x86_64_tar")")
sha256_aarch64=$(sum_of "$aarch64_sum_file" "$(basename "$aarch64_tar")")

# The GitHub tag's own source archive (RFC 016 OQ-2): downloaded and hashed here, never trusted from a
# caller-supplied value, since no release asset carries it. BRYGGE_TEST_TAG_ARCHIVE_BASE is a test-only
# override (tools/test-release-tools.sh), the same shape as BRYGGE_TEST_CRATES_DOWNLOAD.
archive_base=${BRYGGE_TEST_TAG_ARCHIVE_BASE:-https://github.com/prikk-vcs/brygge/archive/refs/tags}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
src_archive="$work/brygge-$version.tar.gz"
curl --silent --show-error --fail --location --retry 3 --retry-all-errors \
    --user-agent "brygge-release-tools (https://github.com/prikk-vcs/brygge)" \
    --output "$src_archive" "$archive_base/$version.tar.gz" || {
    echo "aur-render: cannot download the tag archive for $version from $archive_base" >&2
    exit 1
}
sha256_src=$(sha256sum "$src_archive" | cut -d' ' -f1)

render() { # render <pkg> <sed-script>
    local pkg=$1 script=$2
    mkdir -p "$out/$pkg"
    sed "$script" "$repo/packaging/aur/$pkg/PKGBUILD.in" >"$out/$pkg/PKGBUILD"
    if command -v makepkg >/dev/null 2>&1; then
        (cd "$out/$pkg" && makepkg --printsrcinfo) >"$out/$pkg/.SRCINFO"
    else
        echo "aur-render: makepkg is not installed; skipping $pkg/.SRCINFO (rendered inside the archlinux container in CI)" >&2
    fi
}

render brygge "s/@VERSION@/$version/; s/@PKGREL@/$pkgrel/; s/@SHA256_SRC@/$sha256_src/"
render brygge-bin "s/@VERSION@/$version/; s/@PKGREL@/$pkgrel/; s/@SHA256_X86_64@/$sha256_x86_64/; s/@SHA256_AARCH64@/$sha256_aarch64/"

echo "rendered brygge and brygge-bin for $version-$pkgrel into $out"
