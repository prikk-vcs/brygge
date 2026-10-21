#!/usr/bin/env bash
# Runs as root inside the pinned archlinux:base-devel container (RFC 016 D-2) — makepkg refuses to run as
# root for any mode, including --printsrcinfo, so this creates an unprivileged build user first and does
# the actual work as that user.
#
# Usage: tools/aur-in-container.sh <render-check|build> <dir> [dist-dir]
#   <dir> holds a `brygge/` and a `brygge-bin/` subdirectory, each a rendered package (PKGBUILD, and for
#   `brygge-bin` its extracted source already in place — see tools/aur-render.sh).
#   render-check: makepkg --printsrcinfo (writes .SRCINFO) and namcap on the PKGBUILD, for each package.
#                 No dependency is installed beyond namcap itself; no package is built. The per-change CI
#                 job runs this on a synthetic render — it never touches a real binary or network beyond
#                 the render's own fixtures. <dist-dir> is not used.
#   build:        installs each package's own build dependencies, then makepkg --cleanbuild (runs
#                 check() for `brygge`) and namcap on the built package too. release.yml's `aur` job runs
#                 this on a real render, after the binaries' attestations are verified. <dist-dir> is then
#                 required: it holds the release's own `brygge-$CARCH-unknown-linux-gnu.tar.gz` archives
#                 (already attestation-verified by the caller), used to prove brygge-bin's installed
#                 /usr/bin/brygge is byte-for-byte the attested binary (review 084 D-2), not something
#                 makepkg's build rewrote.
set -euo pipefail

if [ $# -lt 2 ] || [ $# -gt 3 ] || { [ "$1" != render-check ] && [ "$1" != build ]; }; then
    echo "usage: tools/aur-in-container.sh <render-check|build> <dir> [dist-dir]" >&2
    exit 2
fi
mode=$1
dir=$2
dist=${3:-}
if [ "$mode" = build ] && [ -z "$dist" ]; then
    echo "aur-in-container: build mode needs <dist-dir>, the release's verified tar.gz archives" >&2
    exit 2
fi
[ -d "$dir/brygge" ] && [ -d "$dir/brygge-bin" ] || {
    echo "aur-in-container: $dir must hold a brygge/ and a brygge-bin/ directory" >&2
    exit 2
}
if [ "$(id -u)" -ne 0 ]; then
    echo "aur-in-container: run this as root (it creates the unprivileged build user itself)" >&2
    exit 2
fi

pacman -Sy --noconfirm --needed namcap

useradd --create-home --shell /bin/bash builder
passwd --delete builder >/dev/null
chown -R builder:builder "$dir"

as_builder() { runuser -u builder -- bash -lc "$1"; }

for pkg in brygge brygge-bin; do
    echo "::group::$pkg: render"
    as_builder "cd '$dir/$pkg' && makepkg --printsrcinfo >.SRCINFO"
    as_builder "cd '$dir/$pkg' && namcap PKGBUILD"
    echo "::endgroup::"
done

if [ "$mode" = build ]; then
    # Review 083 C-1: `cargo` is a virtual package with two providers in `extra` — `rust` and `rustup` —
    # and `pacman -S --noconfirm` would otherwise pick between them silently (`rustup` installs no
    # toolchain at all, so the build would fail, or download one, unpinned). `rust` is this job's own
    # choice of provider for the PKGBUILD's declared `cargo`; it is not itself a dependency the PKGBUILD
    # lists, so it is installed here, separately from the declared set below, not added to it.
    pacman -S --noconfirm --needed rust
    if ! pacman -Qo "$(command -v cargo)" | grep -q '^/usr/bin/cargo is owned by rust '; then
        echo "aur-in-container: cargo did not resolve to the rust package" >&2
        exit 1
    fi

    # Review 082 F-5: install exactly what the rendered .SRCINFO files declare (depends, makedepends,
    # checkdepends), nothing hand-written here — the PKGBUILD stays the single source of truth, so this
    # container builds the way a real AUR user's own makepkg does. (A hand-written list once forgot
    # `git`, which check() genuinely needs; a declared-but-unlisted dependency can't drift out of sync
    # again, because there is no second list to drift from.)
    mapfile -t deps < <(
        grep -h -E '^[[:space:]]*(depends|makedepends|checkdepends) = ' \
            "$dir"/brygge/.SRCINFO "$dir"/brygge-bin/.SRCINFO |
            sed -E 's/^[[:space:]]*[a-z]+ = //' | sort -u
    )
    if [ "${#deps[@]}" -gt 0 ]; then
        pacman -S --noconfirm --needed "${deps[@]}"
    fi
fi

for pkg in brygge brygge-bin; do
    if [ "$mode" = build ]; then
        echo "::group::$pkg: build"
        as_builder "cd '$dir/$pkg' && makepkg --cleanbuild --noconfirm"
        as_builder "cd '$dir/$pkg' && namcap ./*.pkg.tar.zst"
        echo "::endgroup::"
    fi
done

if [ "$mode" = build ]; then
    # Review 084 D-2: brygge-bin's whole point is "the release's attested binary" — check that on every
    # build, not just by keeping options=('!strip' '!debug') in the template and hoping. Compares the
    # binary installed by the built package against the one inside the verified release tarball, byte
    # for byte.
    echo "::group::brygge-bin: binary identity against the attested release tarball"
    arch=$(uname -m)
    release_tar="$dist/brygge-$arch-unknown-linux-gnu.tar.gz"
    [ -f "$release_tar" ] || {
        echo "aur-in-container: no $release_tar in $dist" >&2
        exit 1
    }
    shopt -s nullglob
    pkgfiles=("$dir"/brygge-bin/*.pkg.tar.zst)
    shopt -u nullglob
    if [ "${#pkgfiles[@]}" -ne 1 ]; then
        echo "aur-in-container: expected exactly one brygge-bin package, found ${#pkgfiles[@]}" >&2
        exit 1
    fi
    work=$(mktemp -d)
    tar --zstd -xf "${pkgfiles[0]}" -C "$work" usr/bin/brygge
    mkdir "$work/release"
    tar -xzf "$release_tar" -C "$work/release" brygge
    installed_sum=$(sha256sum "$work/usr/bin/brygge" | cut -d' ' -f1)
    release_sum=$(sha256sum "$work/release/brygge" | cut -d' ' -f1)
    rm -rf "$work"
    if [ "$installed_sum" != "$release_sum" ]; then
        echo "aur-in-container: brygge-bin's installed /usr/bin/brygge does not match the attested release binary" >&2
        echo "  installed (${pkgfiles[0]}): $installed_sum" >&2
        echo "  release ($release_tar):     $release_sum" >&2
        exit 1
    fi
    echo "brygge-bin: installed /usr/bin/brygge matches the attested release binary ($installed_sum)"
    echo "::endgroup::"
fi
