# RFC 012 — Release automation: tag-triggered, owner-approved, verified publication

**Status.** **Accepted (2026-09-24)** by the owner. OQ-1 and OQ-4 were accepted as recommended; OQ-2
was accepted with the condition "do not damage v0 productivity" (D-2); OQ-3 was answered by the owner:
include Windows and macOS (D-9, D-10). Handoff: `rfcs/handoffs/012-release-automation/`.
**Amended 2026-09-24 (owner ruling):** the tag ruleset of D-2 is dropped for v0, and released-tag
immutability becomes a governance rule; D-11 (publishing the book on GitHub Pages) is added.

## Summary

- **Today, a brygge release is a manual sequence** of tag, `cargo publish`, install check and GitHub
  release. The 0.1.0 cut showed its two weaknesses:
  - who performs each step was decided by a message, not by a mechanism;
  - every step ran on one person's machine, with one person's long-lived crates.io credential.
- **This RFC moves the cut into GitHub Actions.** The workflow publishes only a tag that passes the full
  gate suite, and only after the owner approves that specific release in GitHub. It publishes to crates.io
  **without a stored secret**, verifies the published result, and creates the GitHub release from the
  CHANGELOG.
- **What stays human:** the owner's authorization, now a recorded approval rather than a message, and the
  architect's tag.

## Constraints

- **Owner authorization** (`GOVERNANCE.md`, "Cutting a release"). The owner authorizes every cut and its
  scope. Automation must make that authorization *mechanical and recorded*, never bypass it.
- **The supply chain** (threat model T-4, INV-4). The release pipeline is the first place brygge holds a
  publish credential. A compromised workflow, action or token could publish a malicious `brygge` to every
  user of `cargo install`. The pipeline must therefore:
  - hold the least privilege, for the shortest time;
  - use no long-lived secret where a short-lived one exists;
  - pin third-party code it runs with credentials.
- **Gates (CI-enforced, MSRV 1.85).** A release is the gated tree, re-verified on the exact tagged commit,
  not whatever happened to be green on `main`.
- **Irreversibility.** crates.io versions cannot be deleted. A failure after a partial publication must be
  safe to re-run.

## Decisions

### D-1 — Trigger and preconditions

- **Triggers:**
  - a pushed tag matching `[0-9]+.[0-9]+.[0-9]+` (bare versions, per the release rules);
  - `workflow_dispatch` with a `tag` input, for re-running a cut safely (D-5) and for 0.1.0's GitHub
    release (OQ-4).
- **A `verify` job** runs first, on the tagged commit, and fails the release on any of the following:
  - **the tag:** it is annotated; it points at a commit reachable from `origin/main`; and it equals the
    workspace version in `Cargo.toml`;
  - **the CHANGELOG:** it has exactly one `## [X.Y.Z]` heading for the tag;
  - **the gates:** the full gate suite, exactly as `ci.yml` runs it, on MSRV 1.85 and `--locked` (fmt,
    clippy `-D warnings`, test, `cargo deny`, `cargo audit`, the IR-isolation check, the link check);
  - **packaging:** `cargo publish --workspace --dry-run --locked` succeeds.

### D-2 — The owner's authorization is a GitHub environment approval

- The publishing jobs run in a GitHub **environment named `release`**, with the owner as its **required
  reviewer**.
- After `verify` passes, the run pauses until the owner approves *this* run in GitHub. That approval is
  the recorded authorization of the cut and its scope.
- The architect pushes the tag (`GOVERNANCE.md`); the owner approves the run; nobody else can make a
  publication happen.
- **Released tags: a governance rule, not a mechanism** *(amended 2026-09-24, owner ruling: "rules
  should meet the needs")*.
  - The ruleset first proposed here is **dropped for v0.** With one maintainer, and crates.io versions
    that cannot change anyway, a moved tag could only put git out of step with crates.io, and each
    published crate records its commit in `.cargo_vcs_info.json` regardless.
  - The ruleset's real cost falls exactly when a release attempt fails *before* publishing: the tag must
    be deleted and re-created, and a ruleset blocks that.
  - **The rule** (`GOVERNANCE.md`): once a version is published, its tag is never moved or deleted. A tag
    whose release failed before anything was published may be deleted and re-created, on the owner's
    go-ahead.
  - A ruleset can be reconsidered when the project gains other maintainers.
- **No branch protection, required reviews or required status checks** are added either. Day-to-day
  pushes, other tags and CI are untouched; the approval gate applies only to runs that publish.
- **An honest limit.** The architect and the implementer act through the owner's own git and `gh`
  credentials, so GitHub cannot tell them apart from the owner.
  - An agent holding the owner's `gh` token could technically approve a pending deployment.
  - The boundary therefore rests on `GOVERNANCE.md` and the agents' standing instructions: **no agent
    ever approves a `release` run.** Optionally, and recommended when convenient, agents can be given a
    fine-grained token without the Actions/Deployments write permission, so the gate holds mechanically
    against agents too. This is recorded as a residual in `brygge-03`.

### D-3 — crates.io without a stored secret (trusted publishing)

- Publish with **crates.io trusted publishing** (OIDC): the job requests a short-lived crates.io token for
  this workflow and this environment, through the official `rust-lang/crates-io-auth-action`, and the
  token expires after the job.
- No crates.io token is ever stored in the repository.
- **Prerequisites:**
  - each crate must already exist, which it does since 0.1.0;
  - the owner configures, once per crate on crates.io, this repository, `release.yml` and environment
    `release` as its trusted publisher.
- `id-token: write` is granted to the publish job only.

### D-4 — Least privilege and pinned code

- **The default is `permissions: contents: read`.** Each job widens only what it needs:
  - `id-token: write` for the crates.io publish job;
  - `contents: write` for the GitHub-release job;
  - nothing else.
- **Third-party actions in the publishing jobs are pinned to a full commit SHA**, with the version in a
  comment. The rest of `ci.yml` may keep version tags. The actions to pin are
  `rust-lang/crates-io-auth-action`, `dtolnay/rust-toolchain` and `actions/checkout`.
- **No other third-party action** runs with a credential. The GitHub release uses the `gh` CLI that the
  runner provides.

### D-5 — Publication that is safe to re-run

- Publish crate by crate, in dependency order: `brygge-ir`; then `brygge-decode-cvs`, `-git`, `-hg` and
  `-svn`; then `brygge`.
- **Skip a crate** whose version is already on crates.io (checked against the crates.io index before
  each publish).
  - A re-run after a partial failure then completes the rest, and never fails on what is already done.
  - A dispatch for an already-published tag publishes nothing.
- **Rate limits:** on crates.io's rate limit (HTTP 429), wait for the time crates.io states, up to a
  bound, then retry once. Only a first publication of several *new* crates meets it.
- Each crate is published from the tagged checkout, with `--locked`.

### D-6 — Verify what was published, then release

- **After publishing**, a job runs `cargo install --locked brygge --version X.Y.Z` from crates.io, and
  checks that:
  - `brygge --version` reports `X.Y.Z` and the IR contract version;
  - a smoke test passes: decode and verify a small generated Git repository.
- **Byte-identity:** the job downloads each published `.crate` and compares its files with the tagged
  source, as the architect did by hand for 0.1.0. A mismatch fails the run loudly. crates.io cannot be
  rolled back, but a mismatch must never pass silently.
- **Only then** does the GitHub release get created:
  - `gh release create <tag>`;
  - its notes are the `[X.Y.Z]` section of `CHANGELOG.md`, up to its "detailed changes" line, extracted
    by a small script under `tools/` with its own test.

### D-7 — A scheduled advisory check

- Add `security-audit.yml`, following prikk's: a weekly `cargo audit` against a fresh advisory database,
  plus `workflow_dispatch`, with `contents: read`.
- **Why:** advisories arrive with nobody touching the code. The `faster-hex` advisory already present
  (RUSTSEC-2026-0306, informational, via `gix-hash`) is the kind of thing it tracks.

### D-8 — Governance follows the mechanism

- `GOVERNANCE.md` "Cutting a release" becomes:
  - the architect prepares and pushes the tag, on the owner's go-ahead;
  - the owner approves the `release` run in GitHub;
  - the workflow executes the publication;
  - the architect checks the run and reports.
- The manual procedure stays documented as the **fallback** (the workflow is unavailable, or crates.io's
  trusted publishing is down), still executed by the architect, with the owner's per-step confirmation.

### D-11 — The book on GitHub Pages *(added 2026-09-24, at the owner's request)*

- **What is published:** `docs/src/` (the mdbook of `SUMMARY.md`: the user guides, the references, the
  design set, the development documents) is built and published at
  `https://prikk-vcs.github.io/brygge/`. The owner has enabled Pages with GitHub Actions as its source.
- **`docs/book.toml`** (title `brygge`, `src = "src"`, `site-url = "/brygge/"`, the repository URL for
  the header link). The build output `docs/book/` is gitignored.
- **`docs.yml`** runs on a push to `main` that touches `docs/**` or the workflow itself, and on
  `workflow_dispatch`. It has two jobs:
  - `build`, with `contents: read`: installs mdBook at an **exact** version with `--locked`, builds the
    book, and uploads it with `actions/upload-pages-artifact`;
  - `deploy`, in the `github-pages` environment: runs `actions/deploy-pages`.
  - The workflow's top level is `contents: read`; `pages: write` and `id-token: write` are granted on
    `deploy` only. A `pages` concurrency group lets a newer deploy supersede an older one.
  - Actions use major tags, as in `ci.yml`. No release credential is involved.
- **The book cannot break unnoticed:**
  - `ci.yml`'s `repository` job builds the book on every branch push and pull request, so the deploy is
    never the first build;
  - `tools/check-links.sh` also fails on a relative link in `docs/src/` whose target lies outside
    `docs/src/`, because it would resolve in the repository but 404 on the site. Such links must be
    absolute GitHub URLs. There are none today.
- **Scope:** no mdBook preprocessors or plugins (none are needed), and no landing page. The book's first
  page is its introduction.

### D-9 — CI proves every supported platform

- **The test matrix:**
  - `ci.yml`'s gate job (fmt, clippy `-D warnings`, test, `--locked`) runs on **`ubuntu-latest`**,
    **`ubuntu-24.04-arm`**, **`macos-latest`** (Apple Silicon) and **`windows-latest`**, on stable;
  - the MSRV (1.85) run stays on `ubuntu-latest`, since language-level MSRV breaks are not
    platform-specific;
  - the supply-chain job stays single-platform.
- **Portability is a correctness question, not only a build question.** On Windows the dev team proves,
  with tests that run there:
  - **Paths:** path bytes use `OsStr::as_encoded_bytes` (stable, portable). A Windows name that is not
    valid Unicode is refused as `non-utf8-path`, like a non-UTF-8 Unix name.
  - **Read confinement (C-2c):**
    - a **symlink and a directory junction** under a CVS root are both refused (`symlink-in-repository`);
    - Git's redirected-directory checks treat a junction like a symlink.
    - If `std` does not report junctions as symlinks, detect reparse points explicitly.
  - **Fixtures:** they survive checkout unchanged. A `.gitattributes` marks test data binary, so no line
    endings are converted.
  - **Tests:** Unix-only tests are `cfg(unix)`, with a Windows counterpart wherever the property matters
    (confinement, non-UTF-8 names).
- **Platform-specific residuals** found on the way are recorded in `brygge-03` v0.4.

### D-10 — Binaries for exactly the platforms CI proves, with provenance

- **The release workflow builds** `brygge` for `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`,
  `aarch64-apple-darwin` and `x86_64-pc-windows-msvc`, each on its native runner (the same runners as
  D-9). Each archive is `.tar.gz`, or `.zip` for Windows, and holds the binary, `LICENSE` and a
  `build-info.txt` (target, commit, tag, the build command, `rustc -vV`).
- **Every archive ships** with a SHA-256 checksum and a **GitHub build-provenance attestation**
  (`actions/attest-build-provenance`, pinned by SHA). A user can then verify it with
  `gh attestation verify`. That is a real supply-chain control for the one project in the ecosystem that
  links heavy dependencies.
- **`workflow_dispatch` takes `binaries: true|false`.** 0.1.0's release (OQ-4) is dispatched with
  `false`, because 0.1.0 does not build on Windows. Its release notes point Windows users to 0.1.1.
- **Out of scope:** Intel macOS (`x86_64-apple-darwin`) is not added. Its runners are being retired, and
  a claim we cannot keep testing is not made. It can be added later, if a runner remains.

## Open questions (the owner's)

*Rulings, 2026-09-24:* OQ-1 accepted; OQ-2 accepted with "do not damage v0 productivity" (applied in D-2);
OQ-3 answered by the owner: include Windows and macOS (D-9, D-10); OQ-4 accepted (dispatched with
`binaries: false`, D-10).

- **OQ-1 — How crates.io is authenticated.**
  - **Recommended:** trusted publishing (D-3). No stored secret, and tokens last minutes.
  - **Alternative:** a scoped crates.io API token (publish-update only, these six crates) stored as an
    environment secret. It works, but it is a long-lived secret that can leak.
- **OQ-2 — The approval gate.**
  - **Recommended:** the `release` environment with you as the required reviewer (D-2), plus a tag
    ruleset. *(The ruleset part was later dropped; see D-2's 2026-09-24 amendment.)*
  - **Alternative:** no environment; the tag push alone publishes. That is simpler, but the tag becomes
    the only authorization, and pushing it is not your act.
- **OQ-3 — Prebuilt binaries in GitHub releases.** **Owner: why exclude Windows and macOS?** They
  were never excluded as impossible. The only reason was that brygge's CI had never built or tested on
  them, and a binary for an untested platform would be an unverified claim.
  - **Checking now showed the risk is real.** Cross-compiling `1e3b541`:
    - **macOS compiles;**
    - **Windows does not:** `brygge-decode-cvs/src/scan.rs` uses the Unix-only `OsStrExt::as_bytes`, so
      **brygge 0.1.0 as published cannot be installed on Windows.**
  - **Answer, revised:** Windows and macOS are **in**, the right way. Add them to CI's test matrix (D-9),
    make the code pass there, and ship binaries only for platforms CI builds and tests (D-10).
  - The Windows fix ships as **0.1.1**, the first release cut by this workflow.
- **OQ-4 — 0.1.0's GitHub release.**
  - **Recommended:** once this workflow is merged, dispatch it for tag `0.1.0`. `verify` re-checks the
    tag, the crates are already published and so are skipped (D-5), the D-6 checks run against the live
    crates, and the release is created. 0.1.0 then gets its release through the same audited path as
    every later one.
  - **Alternative:** the architect creates it by hand now, with your confirmation.

## Consequences

- A release needs three acts: the architect's tag, your approval in GitHub, and the workflow's run. It no
  longer depends on anyone's machine or credential.
- The pipeline is a new, security-relevant surface.
  - `brygge-03` gains a control under T-4: *the release pipeline*, which publishes only the gated, tagged
    commit, only on owner approval, with short-lived credentials and pinned actions, and verifies the
    published result.
  - It also gains a residual: *a compromised GitHub account of the owner or architect*. That is mitigated
    by the approval gate and two-factor authentication, which the owner is asked to confirm.
- **Owner setup, once:**
  - create the `release` environment with yourself as reviewer;
  - configure the six crates' trusted publisher on crates.io.

  *(The tag ruleset was dropped by the 2026-09-24 amendment.)*

  The architect writes a step-by-step checklist.
- **The 0.2.0 maintenance items ride along:** `actions/checkout` off Node.js 20, and the Ubuntu 26 runner
  change, in `ci.yml` too.

## Acceptance and verification

- **The workflow** is implemented by the dev team against a handoff, and reviewed by the architect
  against this RFC and `brygge-03`.
- **Proven end to end twice:**
  1. **0.1.1** (the Windows fix): verify passes; the run waits for the owner's approval; the crates are
     published by trusted publishing; the install and byte-identity checks pass; four binaries with
     checksums and attestations are attached; the release is created.
  2. **0.1.0** (OQ-4): dispatched with `binaries: false`; nothing is republished; the release is created.
- **Negative checks** are shown in review:
  - a tag not on `main`, a version mismatch, or a missing CHANGELOG heading each fails `verify`;
  - a job without the environment cannot obtain a crates.io token.
