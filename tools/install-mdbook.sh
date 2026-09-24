#!/usr/bin/env bash
# Install the one pinned mdBook that builds the book (RFC 012 D-11). Both `docs.yml` (the deploy) and
# `ci.yml` (the check on every push and pull request) call this, so they can never build with different
# versions. To move to another version, change MDBOOK_VERSION here, and only here.
#
# The default features are kept: the search index is one of them.
#
# Usage: tools/install-mdbook.sh
set -euo pipefail

MDBOOK_VERSION="0.5.4"

if command -v mdbook >/dev/null 2>&1 && [ "$(mdbook --version)" = "mdbook v$MDBOOK_VERSION" ]; then
    echo "mdbook v$MDBOOK_VERSION is already installed"
    exit 0
fi
cargo install mdbook --version "$MDBOOK_VERSION" --locked
echo "installed $(mdbook --version)"
