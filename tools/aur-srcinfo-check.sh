#!/usr/bin/env bash
# Checks that a rendered brygge-bin .SRCINFO names the right tarball for each architecture (review 110):
# source_x86_64 must point at the x86_64 tarball, source_aarch64 at the aarch64 one. One definition, two
# callers (tools/aur-in-container.sh, tools/test-release-tools.sh), so the pattern cannot drift between
# them.
#
# Review 115 B-1: the first version of this check, inlined in aur-in-container.sh, anchored its pattern
# at column 0 ("^source_x86_64 = ..."). `makepkg --printsrcinfo` indents every field under
# pkgbase/pkgname with a tab, so that pattern never matched *any* real .SRCINFO, correct or not — this
# version tolerates leading whitespace instead.
#
# Usage: tools/aur-srcinfo-check.sh <path-to-.SRCINFO>
# Exit: 0 if both lines are correct; 1 naming the first mismatch; 2 for a usage error.
set -euo pipefail

if [ $# -ne 1 ]; then
    echo "usage: tools/aur-srcinfo-check.sh <path-to-.SRCINFO>" >&2
    exit 2
fi
srcinfo=$1
[ -f "$srcinfo" ] || {
    echo "aur-srcinfo-check: $srcinfo does not exist" >&2
    exit 2
}

if ! grep -qE '^[[:space:]]*source_x86_64 = .*/brygge-x86_64-unknown-linux-gnu\.tar\.gz$' "$srcinfo"; then
    echo "aur-srcinfo-check: $srcinfo's source_x86_64 does not name brygge-x86_64-unknown-linux-gnu.tar.gz" >&2
    exit 1
fi
if ! grep -qE '^[[:space:]]*source_aarch64 = .*/brygge-aarch64-unknown-linux-gnu\.tar\.gz$' "$srcinfo"; then
    echo "aur-srcinfo-check: $srcinfo's source_aarch64 does not name brygge-aarch64-unknown-linux-gnu.tar.gz" >&2
    exit 1
fi
echo "aur-srcinfo-check: $srcinfo names the right tarball for each architecture"
