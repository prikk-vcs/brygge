#!/usr/bin/env bash
# Runs as root inside the pinned archlinux:base-devel container (RFC 016 D-2) — makepkg refuses to run as
# root for any mode, including --printsrcinfo, so this creates an unprivileged build user first and does
# the actual work as that user.
#
# Usage: tools/aur-in-container.sh <render-check|build> <dir>
#   <dir> holds a `brygge/` and a `brygge-bin/` subdirectory, each a rendered package (PKGBUILD, and for
#   `brygge-bin` its extracted source already in place — see tools/aur-render.sh).
#   render-check: makepkg --printsrcinfo (writes .SRCINFO) and namcap on the PKGBUILD, for each package.
#                 No dependency is installed beyond namcap itself; no package is built. The per-change CI
#                 job runs this on a synthetic render — it never touches a real binary or network beyond
#                 the render's own fixtures.
#   build:        installs each package's own build dependencies, then makepkg --cleanbuild (runs
#                 check() for `brygge`) and namcap on the built package too. release.yml's `aur` job runs
#                 this on a real render, after the binaries' attestations are verified.
set -euo pipefail

if [ $# -ne 2 ] || { [ "$1" != render-check ] && [ "$1" != build ]; }; then
    echo "usage: tools/aur-in-container.sh <render-check|build> <dir>" >&2
    exit 2
fi
mode=$1
dir=$2
[ -d "$dir/brygge" ] && [ -d "$dir/brygge-bin" ] || {
    echo "aur-in-container: $dir must hold a brygge/ and a brygge-bin/ directory" >&2
    exit 2
}
if [ "$(id -u)" -ne 0 ]; then
    echo "aur-in-container: run this as root (it creates the unprivileged build user itself)" >&2
    exit 2
fi

pacman -Sy --noconfirm --needed namcap
if [ "$mode" = build ]; then
    # `rust` provides `cargo` (pacman -Si rust), satisfying brygge/PKGBUILD.in's makedepends=('cargo').
    # brygge-bin has no makedepends: its only depends (glibc, gcc-libs) are always in a base image.
    # mercurial, subversion and cvs: not a brygge dependency (its decoders read them directly), but
    # needed for check() to run every test, not skip the hg/svn/cvs suites — matching ci.yml's own gates.
    pacman -S --noconfirm --needed rust mercurial subversion cvs
fi

useradd --create-home --shell /bin/bash builder
passwd --delete builder >/dev/null
chown -R builder:builder "$dir"

as_builder() { runuser -u builder -- bash -lc "$1"; }

for pkg in brygge brygge-bin; do
    echo "::group::$pkg"
    as_builder "cd '$dir/$pkg' && makepkg --printsrcinfo >.SRCINFO"
    as_builder "cd '$dir/$pkg' && namcap PKGBUILD"
    if [ "$mode" = build ]; then
        as_builder "cd '$dir/$pkg' && makepkg --cleanbuild --noconfirm"
        as_builder "cd '$dir/$pkg' && namcap ./*.pkg.tar.zst"
    fi
    echo "::endgroup::"
done
