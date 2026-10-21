# RFC 016 — Arch Linux packaging: `brygge` and `brygge-bin` on the AUR, published by the release workflow

**Status.** **Accepted (2026-10-21)** by the owner ("Yes"). The rulings:
- **OQ-1: the owner's own AUR account (`nabbisen`).** The owner sets the key as a secret in the GitHub Actions
  `release` environment. RR-aur-key's blast radius is therefore every package `nabbisen` maintains: a stated,
  deliberate trade.
- **OQ-2: the GitHub tag's source archive.**
- **OQ-3: automatic at every release,** with the `aur`-only dispatch as the fallback.

Handoffs: `rfcs/handoffs/016-arch-linux-packaging/`. Proposed (2026-10-21) by the architect.
**Scheduled:** in parallel with RFC 015's batch V, so that **0.5.0 is the first release the workflow publishes to the
AUR.**

## Summary

The owner registered two AUR packages, both now placeholders (`0.0.0-1`, maintainer `nabbisen`):
- **`brygge`,** built from source;
- **`brygge-bin`,** the release's prebuilt, attested binary.

They are to be updated by GitHub Actions, as prikk is doing. This RFC adds that to RFC 012's release pipeline: after
the crates and the GitHub release are published, the workflow renders both packages from templates in this
repository, **builds and checks each in a real Arch Linux container,** and pushes them to the AUR.

## Facts that shape the design (checked by the architect)

- **The AUR has no trusted publishing** (no OIDC). Pushing to `ssh://aur@aur.archlinux.org/<pkg>.git` needs an
  **SSH key** registered to an AUR account, so it is **brygge's first stored secret.** C-4f says today "crates.io
  trusted publishing with no stored secret", so the threat model must change honestly.
- **An AUR SSH key can push to every package its account maintains.** Its blast radius is the account, not one
  package (OQ-1).
- **GitHub does not start a new workflow for events created with `GITHUB_TOKEN`** (except `workflow_dispatch` and
  `repository_dispatch`). The release is created by `release.yml` itself, so the AUR step must be **a job in
  `release.yml`**, not a separate workflow fired by `release: published`.
- **`makepkg` refuses to run as root,** and `.SRCINFO` must come from `makepkg --printsrcinfo`. So the job runs in
  an **`archlinux:base-devel` container** (pinned by digest), with an unprivileged build user.

## Decisions

### D-1 — Two packages, one source of truth

- **The templates live in this repository:** `packaging/aur/brygge/PKGBUILD.in` and
  `packaging/aur/brygge-bin/PKGBUILD.in`, with placeholders for the version and the checksums only.
- **`tools/aur-render.sh <X.Y.Z> <out-dir>`** renders `PKGBUILD` and `.SRCINFO` for both, from the release's own
  files:
  - **`brygge-bin`:** the release's Linux binaries for `x86_64` and `aarch64`, with the checksums from the
    release's `.sha256` assets. **CI verifies each binary's attestation (`gh attestation verify`) before
    rendering**, so the AUR never carries a binary the release did not attest;
    - `provides=('brygge')`, `conflicts=('brygge')`;
    - the license file goes into `/usr/share/licenses/brygge-bin/`;
  - **`brygge`:** a source build (OQ-2), following the Arch Rust package guidelines:
    - `prepare()` runs `cargo fetch --locked`;
    - `build()` runs `cargo build --frozen --release`;
    - `check()` runs `cargo test --frozen`;
    - it installs only the `brygge` binary, and the license;
    - `makedepends=('cargo')`; `optdepends=('subversion: decode a local SVN repository (dump files need nothing)')`.
      Git, Mercurial and CVS need no external tool: brygge reads them directly.

### D-2 — The `aur` job, in `release.yml`

- **When it runs:** after `github-release` succeeds, in the `release` environment (where the SSH key lives).
- **What it does:**
  1. it renders both packages;
  2. **it builds each with `makepkg` in the container** (an unprivileged user; `brygge` runs its own `check()`),
     and lints them with `namcap`;
  3. only if both pass, it pushes each to its AUR repository, as one commit: "Update to X.Y.Z".
- **SSH safety:**
  - **the AUR host key is pinned** (a `known_hosts` entry with the AUR's published fingerprints, committed in the
    repository; never trust-on-first-use);
  - the key is written to a file with mode 0600 inside the job, and removed afterwards;
  - no other job can see it.
- **Failure:** the crates and the GitHub release are already published, and they stay published. The `outcome`
  job reports **"released; AUR not updated"** as a distinct failure. The architect then re-runs the job alone,
  through `workflow_dispatch` with the tag (the `aur`-only path), after a fix.

### D-3 — Packaging-only fixes, and the version

- `pkgrel` is `1` for each new version. **A packaging-only fix** (a PKGBUILD change, the same brygge version)
  bumps `pkgrel` through the `aur`-only dispatch, with a `pkgrel` input. It is never a brygge release.
- The templates are checked per push only when they change: a CI job on `packaging/**` and the render script
  renders a test version and runs `makepkg --printsrcinfo` and `namcap`. **Triggered by a change, never
  scheduled** (GOVERNANCE).

### D-4 — The threat model

- **C-4f is restated:**
  - crates.io still uses trusted publishing, with no stored secret;
  - **the AUR uses one stored SSH key**, scoped to the `release` environment, with a pinned host key;
  - its account and blast radius are per OQ-1.
- **A new residual, RR-aur-key:** a leaked key could push a malicious PKGBUILD to every package its AUR account
  maintains. The mitigations: OQ-1's account choice, environment scoping, key rotation on any suspicion, and the
  attestation check before rendering.

## Open questions (the owner's)

- **OQ-1 — Which AUR account the CI key belongs to.**
  - **Recommended:** a **dedicated bot account** (for example `prikk-vcs-bot`), added as a **co-maintainer** of only
    the prikk-vcs packages. A leaked key then reaches those packages, not everything `nabbisen` maintains.
    - If prikk uses the same approach, **one bot account for both projects** is simpler to manage. Its blast
      radius is both projects' packages, which is a deliberate trade.
  - **Alternative:** a key on `nabbisen` itself. It is simpler, but the blast radius is every package you maintain.
- **OQ-2 — What the `brygge` (source) package builds from.**
  - **Recommended:** the **GitHub tag's source archive**: the repository as tagged, with `Cargo.lock` and the
    test suite, so `check()` runs brygge's own tests.
  - **Alternative:** the crates.io `.crate` file. It is smaller, but it omits the workspace's other crates' tests.
- **OQ-3 — Automatic or manual.**
  - **Recommended: automatic at every release,** with the `aur`-only dispatch as the manual fallback.
  - **Alternative:** manual only. That is one more per-release step for the architect.

## For alignment with prikk (proactive, not prescriptive)

- prikk is taking the same approach. **If the owner can place prikk's current AUR workflow** under
  `.git-exclude/tmp/`, the architect will align the conventions where they make sense: the account, the job
  shape, and the commit message.
- **brygge does not copy prikk's design blindly,** and does not push its own on prikk.

## Proof (for the implementation batch)

- A dry run with a test version renders both packages and builds them in the container (`makepkg`, `namcap`,
  `check()` passing), with no push.
- The pinned host key is checked.
- The push is proven at 0.5.0 itself. Before that, the job runs with a `dry_run` input that stops before `git push`.
