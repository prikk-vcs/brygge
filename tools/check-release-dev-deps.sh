#!/usr/bin/env bash
# Guard against the class of bug in review 059 F-1 (RFC 014 D-1): a dev-dependency that names a `version`
# on a crate that is `publish = false` is kept in the *published* manifest of whatever depends on it, and
# crates.io requires that version to exist there — which a `publish = false` crate never will. The real
# `cargo publish` fails at the first such crate; `cargo publish --workspace --dry-run` does not catch it,
# because it resolves workspace members locally rather than from the registry. This check reads the
# dependency graph through `cargo metadata` instead of trying to reproduce that distinction by hand.
#
# Fails if any workspace member declares a dev-dependency, with a real version requirement (not a bare
# path, i.e. not `req == "*"`), on another workspace member whose own `publish` is `[]` (`publish = false`
# in its `Cargo.toml`).
#
# Usage: tools/check-release-dev-deps.sh [workspace-dir]
#   workspace-dir: a directory containing a Cargo workspace to check (default: this repository). Only
#   `tools/test-release-tools.sh` passes one, to check a scratch workspace that reproduces the bug shape.
# Needs: cargo, python3.
set -euo pipefail

cd "${1:-"$(dirname "${BASH_SOURCE[0]}")/.."}"

metadata_file=$(mktemp)
trap 'rm -f "$metadata_file"' EXIT
cargo metadata --format-version 1 --no-deps > "$metadata_file"

python3 - "$metadata_file" << 'PYEOF'
import json
import sys

with open(sys.argv[1]) as f:
    meta = json.load(f)
packages = {p["name"]: p for p in meta["packages"]}
unpublishable = {name for name, p in packages.items() if p.get("publish") == []}

failures = []
for pkg in meta["packages"]:
    for dep in pkg["dependencies"]:
        if dep.get("kind") != "dev":
            continue
        if dep.get("path") is None:
            continue
        if dep.get("req") == "*":
            continue
        if dep["name"] in unpublishable:
            failures.append(
                f"{pkg['name']} depends (dev, path) on {dep['name']} with version requirement "
                f"'{dep['req']}', but {dep['name']} is publish = false and will never exist on crates.io "
                f"under that requirement"
            )

if failures:
    print("FAILED: a versioned dev-dependency on a publish = false crate (review 059 F-1's class):")
    for f in failures:
        print(f"  - {f}")
    print()
    print("Make the dependency path-only (drop `version`); add `allow-wildcard-paths = true` under")
    print("[bans] in deny.toml if cargo-deny's wildcard check then objects.")
    sys.exit(1)

print(f"OK: no versioned dev-dependency on a publish = false crate ({len(unpublishable)} such crate(s)).")
PYEOF
