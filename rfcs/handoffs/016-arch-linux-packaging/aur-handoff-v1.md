# Handoff — RFC 016: `brygge` and `brygge-bin` on the AUR, published by `release.yml` (batch P)

**Governing:** RFC 016, accepted 2026-10-21. The rulings: OQ-1, the owner's account `nabbisen`, with the key in the
`release` environment; OQ-2, the GitHub tag archive; OQ-3, automatic, with the `aur`-only dispatch as fallback.
**Independent of RFC 015's batch V.** One review request, **on a topic branch** (it changes `release.yml`).
**Target:** 0.5.0 is the first release the workflow publishes to the AUR.

## 0. First, commit the architect's files on their own

Commit as "RFC 016 accepted (Arch Linux packaging); handoff":
- `rfcs/done/016-arch-linux-packaging.md`;
- `rfcs/handoffs/016-arch-linux-packaging/`;
- `rfcs/README.md`;
- `ROADMAP.md`.

## 1. The templates and the renderer

- **`packaging/aur/brygge-bin/PKGBUILD.in`:**
  - `arch=('x86_64' 'aarch64')`, with a source per architecture (`source_x86_64`, `source_aarch64`), which are the
    release's `brygge-<target>.tar.gz` assets;
  - `sha256sums_x86_64` and `sha256sums_aarch64`, from the release's `.sha256` files;
  - `package()` installs `/usr/bin/brygge` and the license (`/usr/share/licenses/brygge-bin/LICENSE`);
  - `provides=('brygge')`, `conflicts=('brygge')`.
- **`packaging/aur/brygge/PKGBUILD.in`:**
  - the source is `https://github.com/prikk-vcs/brygge/archive/refs/tags/$pkgver.tar.gz`, with its SHA-256
    computed by the renderer;
  - `makedepends=('cargo')`;
  - `prepare()` runs `cargo fetch --locked --target "$(rustc -vV | sed -n 's/host: //p')"`;
  - `build()` runs `cargo build --frozen --release -p brygge`;
  - `check()` runs `cargo test --frozen --workspace`;
  - `package()` installs the binary and the license;
  - `optdepends=('subversion: decode a local SVN repository (dump files need nothing)')`.
  - Follow the Arch Wiki's "Rust package guidelines". Cite the page in the review request, and say where you
    deviate, and why.
- **`tools/aur-render.sh <X.Y.Z> <assets-dir> <out-dir>`:**
  - it fills in the version and the checksums only, and refuses a missing asset or a malformed version;
  - **it reads the checksums from the release's own `.sha256` files,** after the caller has verified each binary's
    attestation (§2). It computes the tag archive's SHA-256 itself;
  - it writes `PKGBUILD` and `.SRCINFO` (the latter from `makepkg --printsrcinfo`, inside the container).
- **`pkgrel`:** `1`, unless the `aur`-only dispatch passes another (RFC 016 D-3).

## 2. The `aur` job, in `release.yml`

- **When it runs:** after `github-release` succeeds, in the `release` environment, on `ubuntu-24.04` (pinned, as the
  build is), running an `archlinux:base-devel` container **pinned by digest**.
- **The steps:**
  1. download the release's Linux assets and their `.sha256` files, and **`gh attestation verify` each binary**
     (as the architect does after each release). A failure stops the job;
  2. render both packages;
  3. as an **unprivileged user** in the container, run `makepkg --cleanbuild` for each (`brygge` runs its
     `check()`), then `namcap` on each `PKGBUILD` and each built package. Report the `namcap` output; fix what it
     flags, or justify each exception in the request;
  4. **only if both pass:** clone `ssh://aur@aur.archlinux.org/brygge.git` and `…/brygge-bin.git`, replace the
     `PKGBUILD` and the `.SRCINFO`, commit "Update to X.Y.Z" (or "Packaging fix, X.Y.Z-N"), and push.
- **SSH:**
  - the secret is **`AUR_SSH_PRIVATE_KEY`** in the `release` environment (the owner sets it; see §5). Write it to
    a mode-0600 file, use it through `GIT_SSH_COMMAND` with `-o IdentitiesOnly=yes`, and **pin the host key**: a
    committed `packaging/aur/known_hosts`, holding the AUR's published host keys. **Cite where you took them from**
    (the AUR's own published fingerprints), and **never** use `StrictHostKeyChecking=no` or `ssh-keyscan` at run
    time;
  - remove the key file at the end of the job, even on failure;
  - the commit author is the owner's AUR identity (the name and email on `nabbisen`'s AUR account, given by the
    owner; or a neutral "brygge release workflow" identity if the owner prefers). Ask the owner.
- **The `outcome` job:** add the AUR result. An AUR failure after a published release is reported distinctly as
  **"released; AUR not updated"**, never a green run.
- **The `aur`-only dispatch:** a `workflow_dispatch` input (for example `aur_only: true`, with `tag`, and an optional
  `pkgrel`) runs only the `aur` job, for an existing release. Dispatching stays the architect's (GOVERNANCE).
- **A `dry_run` input:** it stops before `git push`, so the job can be proven before 0.5.0.

## 3. Per-change checks (triggered, never scheduled)

- **A CI job on changes** to `packaging/**`, `tools/aur-render.sh` and the workflow files:
  - render a test version (with fake assets and checksums);
  - run `makepkg --printsrcinfo` and `namcap` on the `PKGBUILD` in the same pinned container;
  - **no build and no push.**
- **`tools/test-release-tools.sh`** gains cases for `aur-render.sh`: a good render, a missing asset refused, a bad
  version refused.

## 4. Proof

1. **The dry run, on the topic branch,** against the **existing 0.4.0 release** (`workflow_dispatch`,
   `aur_only: true`, `dry_run: true`, dispatched by the architect at your request):
   - the attestations are verified;
   - both packages render, **build in the container**, pass `check()` and `namcap`;
   - it stops before the push.
   - Report the run.
2. **The per-change job** runs on the branch, since `packaging/**` changes.
3. **`known_hosts`:** state its source, and show that a host key mismatch fails the connection (a local test
   against a deliberately wrong `known_hosts`, with no push).
4. **The usual gates,** and the release-tool checks.

## 5. For the owner (one-time setup, before 0.5.0): put it in `docs/src/development/releasing.md`

Add an "AUR" step to "One-time setup":
1. generate a key pair for this purpose only (`ssh-keygen -t ed25519 -C "brygge release workflow" -f aur_brygge`);
2. add the **public** key to the `nabbisen` AUR account (My Account → SSH Public Key);
3. add the **private** key as the secret **`AUR_SSH_PRIVATE_KEY`** in the GitHub `release` environment;
4. delete the local private key file afterwards (it lives only in the secret);
5. if the key is ever suspected leaked: remove it from the AUR account first, then replace the secret.

## 6. CHANGELOG `[Unreleased]`

- **Added:** "Arch Linux packages: `brygge` (built from source) and `brygge-bin` (the release's attested binary) on
  the AUR, published by the release workflow after each release (RFC 016)."

## 7. Review request

`.git-exclude/review-requests/054-aur-packaging.md`, with the topic-branch routine.
- The architect revises the threat model (C-4f, and RR-aur-key) at the 0.5.0 cut.
- Do not edit `brygge-03`.
