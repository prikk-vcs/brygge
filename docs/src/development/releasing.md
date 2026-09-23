# Releasing brygge

How a brygge release is cut: `.github/workflows/release.yml`, specified by RFC 012. Who does what is set
by [`GOVERNANCE.md`](handoffs/GOVERNANCE.md) ("Cutting a release"):
- the **owner** authorizes each release by approving its run in GitHub;
- the **architect** prepares the release and pushes its tag;
- the **implementer** never tags, approves or publishes.

## One-time setup (the owner; needs repository-admin and crates.io-owner rights)

Do these once, before the first automated release. Each is reversible in the same settings page.

1. **Create the `release` environment.** GitHub → the repository → *Settings → Environments → New
   environment* → name it `release`.
   - **Required reviewers:** yourself.
   - **Prevent self-review:** leave it **off**. Release tags are pushed through your own account, so with
     it on you could not approve your own release.
   - **Deployment branches and tags:** *Selected branches and tags*. Add the tag rule `*.*.*` and the
     branch `main`; `main` is needed for re-runs dispatched from the Actions tab. This setting is part of
     the security design: a dispatch runs the release tools of the commit it was dispatched from, and this
     rule means that commit can only be `main` or a release tag.
   - Add no secrets. The workflow needs none.
2. **Protect released tags.** *Settings → Rules → Rulesets → New ruleset → New tag ruleset*, named
   `release tags`, with enforcement *Active*.
   - **Target tags:** include the pattern `*.*.*`.
   - **Rules:** tick **Restrict updates** and **Restrict deletions**. Leave *Restrict creations* **off**;
     creating the tag is how a release starts.
   - Leave *Bypass list* empty.
   - This rule makes a released tag immutable. It does not touch branches, other tags, pushes or CI.
3. **Trust the workflow on crates.io.** For each of the six crates (`brygge-ir`, `brygge-decode-cvs`,
   `brygge-decode-git`, `brygge-decode-hg`, `brygge-decode-svn`, `brygge`): crates.io → the crate →
   *Settings → Trusted Publishing → Add* → GitHub, with:
   - repository owner `prikk-vcs`;
   - repository `brygge`;
   - workflow filename `release.yml`;
   - environment `release`.

   With this in place, the workflow gets a crates.io token that lasts minutes, and no crates.io token is
   stored anywhere.
4. **Optional, recommended when convenient:** give the AI agents working on brygge a GitHub token
   **without** the *Actions* and *Deployments* write permissions (a fine-grained personal access token).
   GitHub then itself prevents an agent from approving a `release` run. Without it, that rule rests on
   `GOVERNANCE.md` alone.
5. **Account safety:** two-factor authentication on the GitHub and crates.io accounts that hold these
   rights.

## Cutting a release

1. **The architect prepares the cut commit:**
   - the version bump in `Cargo.toml` (the workspace version and the workspace dependency versions);
   - `CHANGELOG.md`: `[Unreleased]` → `[X.Y.Z] — <date>`, headed by the release notes;
   - status lines, where they name a version.
2. **The implementer commits and pushes it,** and reports its CI result (all four platforms green).
3. **The architect,** with the owner's go-ahead, pushes the annotated tag:
   `git tag -a X.Y.Z -m "brygge X.Y.Z"`, then `git push origin X.Y.Z`.
4. **`release.yml` runs.** `verify` re-checks the tag and runs the full gates on it, and the binaries are
   built. Then the run **waits for the owner's approval**, once, at the "publish to crates.io" job, in
   GitHub (*Actions → the run → Review deployments → release → Approve*). Nothing is published before
   that approval, and nothing after it needs another.
5. **After approval,** the workflow:
   - publishes the crates that are not yet on crates.io, in dependency order;
   - installs `brygge` from crates.io and smoke-tests it;
   - checks the published crates are byte-identical to the tag;
   - creates the GitHub release, with the binaries, their checksums and their provenance attestations.
6. **The architect** checks the run, verifies the release page, and records the release.

**Re-running.**
- *Actions → Release → Run workflow* with the same `tag`. It is safe: crates already published are
  skipped.
- It stops without changing anything if the GitHub release already exists.
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
