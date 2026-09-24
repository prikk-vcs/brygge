# Releasing brygge

How a brygge release is cut: `.github/workflows/release.yml`, specified by RFC 012. Who does what is set
by [`GOVERNANCE.md`](handoffs/GOVERNANCE.md) ("Cutting a release"):
- the **owner** authorizes each release by giving the architect an explicit go-ahead;
- the **architect** prepares the release and pushes its tag;
- the **implementer** never tags or publishes.

## One-time setup (the owner; needs repository-admin and crates.io-owner rights)

*(Released tags are protected by a governance rule rather than a GitHub ruleset, and there is no
per-release approval click: both by owner ruling for v0, RFC 012 D-2 as amended 2026-09-24.)*

Do these once, before the first automated release. Each is reversible in the same settings page.

1. **Create the `release` environment.** GitHub → the repository → *Settings → Environments → New
   environment* → name it `release`.
   - **Required reviewers:** none. The per-release approval gate was removed for v0 after 0.1.1; the
     owner's go-ahead before the tag is the authorization.
   - **Deployment branches and tags:** *Selected branches and tags*. Add the tag rule `*.*.*` and the
     branch `main`; `main` is needed for re-runs dispatched from the Actions tab. This setting is part of
     the security design: a dispatch runs the release tools of the commit it was dispatched from, and this
     rule means that commit can only be `main` or a release tag.
   - Add no secrets. The workflow needs none.
2. **Trust the workflow on crates.io.** For each of the six crates (`brygge-ir`, `brygge-decode-cvs`,
   `brygge-decode-git`, `brygge-decode-hg`, `brygge-decode-svn`, `brygge`): crates.io → the crate →
   *Settings → Trusted Publishing → Add* → GitHub, with:
   - repository owner `prikk-vcs`;
   - repository `brygge`;
   - workflow filename `release.yml`;
   - environment `release`.

   With this in place, the workflow gets a crates.io token that lasts minutes, and no crates.io token is
   stored anywhere.
3. **Account safety:** two-factor authentication on the GitHub and crates.io accounts that hold these
   rights.

## Cutting a release

1. **The architect prepares the cut commit:**
   - the version bump in `Cargo.toml` (the workspace version and the workspace dependency versions);
   - `CHANGELOG.md`: `[Unreleased]` → `[X.Y.Z] — <date>`, headed by the release notes;
   - status lines, where they name a version.
2. **The implementer commits and pushes it,** and reports its CI result (all four platforms green).
3. **The architect asks the owner for the go-ahead,** immediately before the tag, and records it. Then
   the architect pushes the annotated tag: `git tag -a X.Y.Z -m "brygge X.Y.Z"`, then
   `git push origin X.Y.Z`. **Pushing the tag is the act that publishes.**
4. **`release.yml` runs.** `verify` re-checks the tag and runs the full gates on it, and the binaries are
   built. Nothing is published unless all of that passes.
5. **Then the workflow:**
   - publishes the crates that are not yet on crates.io, in dependency order;
   - installs `brygge` from crates.io and smoke-tests it;
   - checks the published crates are byte-identical to the tag;
   - creates the GitHub release, with the binaries, their checksums and their provenance attestations.
6. **The architect** checks the run, verifies the release page, and records the release.

**A failed attempt.** If the run fails *before* `publish to crates.io` (the tag was wrong, a gate
failed), nothing was published. The architect may then delete the tag and re-create it on the fixed
commit, with the owner's go-ahead (`git push --delete origin X.Y.Z`). Once any crate of a version is
published, its tag is never moved or deleted (`GOVERNANCE.md`).

**Re-running.**
- *Actions → Release → Run workflow* with the same `tag`. It is safe: crates already published are
  skipped.
- It stops without changing anything if the GitHub release already exists.
- A release of an older tag is never marked "Latest": only a version higher than every release already on
  GitHub is (`tools/release-latest.sh`).
- The inputs `binaries: false` and `note` are for releases whose binaries cannot or should not be built;
  0.1.0 is the only such release so far.

**Checking a downloaded binary.**
- `sha256sum -c brygge-<target>.tar.gz.sha256`, then
  `gh attestation verify brygge-<target>.tar.gz --repo prikk-vcs/brygge`.
- Or install from source with `cargo install --locked brygge`.

## Manual fallback (only when the workflow cannot run)

The architect executes it, confirming each step with the owner immediately before it:
1. From a clean checkout of the tag, `cargo publish -p <crate> --locked` for each crate not yet
   published, in dependency order.
2. `cargo install --locked brygge --version X.Y.Z`, then a smoke test.
3. `tools/check-published.sh X.Y.Z`.
4. `gh release create X.Y.Z --verify-tag --notes-file <(tools/release-notes.sh X.Y.Z)`.

A first publication of several **new** crates can hit crates.io's new-crate rate limit (HTTP 429). Wait
for the time it states, then continue with the remaining crates. Versions can be yanked, never deleted.
