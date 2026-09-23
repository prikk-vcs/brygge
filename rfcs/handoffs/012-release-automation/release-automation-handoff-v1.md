# Handoff — RFC 012: CI on every supported platform, and an owner-approved release workflow

**Governing RFC:** RFC 012 (release automation), accepted 2026-09-24. Read it first: D-1…D-10 are the
specification, and this handoff turns them into work.

**Why now.**
- brygge 0.1.0 **does not build on Windows** (`brygge-decode-cvs/src/scan.rs` imports the Unix-only
  `std::os::unix::ffi::OsStrExt`). Its fix ships as **0.1.1**, the first release this workflow cuts.
- Until then, releases are manual.

**Roles** (`GOVERNANCE.md`, "Cutting a release"):
- **The dev team:** implements and tests everything below, files the review request, and commits and
  pushes the approved work.
- **The dev team does not:** push a version tag, approve a `release` run, or publish anything, including
  while testing the workflow.
- **The architect:** prepares the 0.1.1 cut commit, pushes the tag, and dispatches the 0.1.0 run.
- **The owner:** does the one-time setup in `docs/src/development/releasing.md`, and approves each run.

---

## 1. Portability (RFC 012 D-9)

1. **Product code.** Replace every `std::os::unix` use outside tests with a portable equivalent.
   - For the CVS scanner that is `OsStr::as_encoded_bytes()` (stable since 1.74; MSRV 1.85 is fine): the
     `,v` suffix test, the lossless `\xNN` display, and the per-component UTF-8 check.
   - On Windows, a name that is not valid Unicode (an unpaired surrogate) then shows as bytes that are not
     UTF-8, and is refused as `non-utf8-path`, exactly as on Unix.
2. **Read confinement on Windows** (threat model C-2c). With tests that run on `windows-latest`, prove:
   - under a CVS root, a **file symlink**, a **directory symlink** and a **directory junction** are each
     refused as `symlink-in-repository`;
   - in Git's checks (a symlinked `.git`, `objects`, `objects/info`, `objects/pack` or pack entry), a
     junction is treated like a symlink.

   If `std`'s `is_symlink()` does not report junctions, detect reparse points explicitly
   (`std::os::windows::fs::MetadataExt::file_attributes`, `FILE_ATTRIBUTE_REPARSE_POINT`), behind
   `cfg(windows)`. Report what `std` actually does, with the test output.
3. **Tests.**
   - Every test using `std::os::unix` is `cfg(unix)`.
   - Where the property matters, a `cfg(windows)` counterpart exists: confinement (item 2), and non-UTF-8
     names (`OsString::from_wide` with an unpaired surrogate).
4. **Fixtures.** Add a `.gitattributes` so test data (the vendored SHA-mbles vector, any committed
   dumpfile or `,v` fixture) is `-text`. No line-ending conversion on a Windows checkout.
5. **Tools and the tools gates.** `tools/check-links.sh` and `tools/check-ir-isolation.sh` stay bash. They
   run in the Linux job only, since they check the repository, not the platform.

## 2. CI matrix (`ci.yml`; RFC 012 D-9)

- **The gate job** (fmt, clippy `-D warnings`, test, all `--locked`) runs on a matrix of `ubuntu-latest`,
  `ubuntu-24.04-arm`, `macos-latest` and `windows-latest`, on stable.
- **The MSRV job** (1.85 clippy and test) stays on `ubuntu-latest`.
- **The supply-chain job and the link check** stay on `ubuntu-latest`.
- **`ci.yml` becomes reusable** (`on: workflow_call` with a `ref` input, plus its existing
  `push`/`pull_request` triggers), so `release.yml` runs **the same gates**, not a copy that can drift.
- **Workflow permissions:** `contents: read` at the top.
- **Maintenance items** from the 0.1.0 report:
  - move `actions/checkout` and `dtolnay/rust-toolchain` to current majors (off Node.js 20);
  - check nothing depends on `ubuntu-latest` being Ubuntu 24 (Ubuntu 26 arrives on 2026-10-19).
- **Tools on the runners.** Tests that need `hg`, `svnadmin` or `git` keep skipping cleanly when the tool
  is absent. Install `mercurial` and `subversion` on the Linux x86_64 job, so the hg and SVN suites really
  run there, and state in the review which suites ran where.

## 3. `release.yml` (RFC 012 D-1…D-6, D-10)

**Triggers:**
- a push of a tag matching `[0-9]+.[0-9]+.[0-9]+`;
- `workflow_dispatch` with inputs `tag` (required), `binaries` (boolean, default `true`) and `note`
  (optional text, §4).
- **No `${{ inputs.* }}` or `${{ github.* }}` expression is interpolated directly into a `run:` script.**
  Pass them through `env:`. This prevents script injection through a crafted input.

The jobs, in order:
1. **`verify`** (no environment, `contents: read`), on the tag's commit (for a dispatch, check out
   `refs/tags/<tag>`). Fail if:
   - the tag is not annotated;
   - the tagged commit is not reachable from `origin/main`;
   - the tag does not equal the workspace version;
   - `CHANGELOG.md` does not have exactly one `## [X.Y.Z]` heading;
   - `cargo publish --workspace --dry-run --locked` fails.

   Then call `ci.yml` (reusable) with `ref` = the tag.
2. **`build`** (matrix of D-10's four targets on their native runners; skipped when `binaries` is
   false):
   - `cargo build -p brygge --release --locked --target <t>`;
   - package it as `.tar.gz`, or `.zip` on Windows, with the binary, `LICENSE` and `build-info.txt`
     (target, commit, tag, the exact build command, `rustc -vV`);
   - write a `.sha256` for each archive;
   - upload them as workflow artifacts.

   prikk's `release.yml` (in `.git-exclude/tmp/prikk-0.46.0/.github-workflows/`) is a good model for the
   per-platform packaging, including the Windows checksum format.
3. **`publish-crates`** (`environment: release`; `permissions: id-token: write, contents: read`):
   - obtain a short-lived token with `rust-lang/crates-io-auth-action`, pinned by commit SHA;
   - for each crate in order (`brygge-ir`, `brygge-decode-cvs`, `brygge-decode-git`, `brygge-decode-hg`,
     `brygge-decode-svn`, `brygge`): skip it if the version is already in the crates.io sparse index,
     otherwise `cargo publish -p <crate> --locked`;
   - on HTTP 429, wait for the stated time (bounded at 15 minutes), then retry once;
   - log exactly which crates were published and which were skipped.
4. **`verify-published`** (`contents: read`):
   - `cargo install --locked brygge --version X.Y.Z` from crates.io into a temporary root;
   - check that `brygge --version` reports X.Y.Z and the IR contract version;
   - smoke-test it: create a two-commit Git repository, `decode`, `verify`, and `verify --against-source`,
     all exit 0 and `pass`;
   - **byte-identity:** for each of the six crates, download the published `.crate`, compare every file
     except the cargo-rewritten manifests with `git show <tag>:crates/<crate>/<path>`, and check that
     `.cargo_vcs_info.json` names the tagged commit. Any difference fails the run.
5. **`github-release`** (`environment: release`; `permissions: contents: write, id-token: write,
   attestations: write`):
   - download the build artifacts, if any;
   - attest each archive with `actions/attest-build-provenance`, pinned by SHA;
   - extract the notes (§4);
   - `gh release create <tag> --verify-tag --title <tag> --notes-file notes.md`, with the archives and
     checksums when `binaries` is true.

   A dispatch for a tag whose release already exists fails in this job with a clear message; it never
   edits an existing release.

**Rules across the workflow:**
- The top-level permission is `contents: read`. Each job widens only what the list above gives it.
- **Every third-party action in a job that holds a credential** (`publish-crates`, `github-release`) is
  pinned to a full commit SHA, with the version in a comment.
- No secrets. Trusted publishing replaces the crates.io token, and `gh` uses the job's `GITHUB_TOKEN`.
- **Do not add** `CARGO_REGISTRY_TOKEN` as a repository or environment secret.

## 4. Tools

- **`tools/release-notes.sh <X.Y.Z>`** prints the `## [X.Y.Z]` section of `CHANGELOG.md`, from after its
  heading up to (not including) the line that begins "The detailed changes". If that line is absent, it
  prints up to the next `## [` heading. It exits non-zero when the section is missing.
  - **Tested in `ci.yml`:** run it for `0.1.0` and assert that the output begins with "**The first
    release.**" and does not contain "The detailed changes".
  - **`workflow_dispatch` has an optional `note` input.** When it is given, its text is appended to the
    notes as a final paragraph. No version is hard-coded in the workflow. The architect uses it for
    0.1.0 ("Windows: 0.1.0 does not build on Windows; use 0.1.1 or later."). The input is plain text;
    pass it to `gh` through a file or an environment variable, never interpolated into a shell command.
- **`tools/check-published.sh <X.Y.Z>`** does the byte-identity comparison of §3.4, so it can also be
  run by hand (the manual fallback).
  - **Tested in `ci.yml`** by running it against `0.1.0`: it must pass, since 0.1.0 is published and
    identical.

## 5. Scheduled advisory check (RFC 012 D-7)

`.github/workflows/security-audit.yml`, following prikk's (in the same directory as above):
- a weekly `cargo audit` against a fresh database, plus `workflow_dispatch`;
- `contents: read`;
- `cargo-audit` installed with `--locked`, not through a third-party action.

## 6. CHANGELOG

- Under `[Unreleased]`, add entries for the Windows fix (**Fixed:** brygge now builds and runs on
  Windows), the CI matrix, and the release workflow.
- Do **not** create a `[0.1.1]` heading or bump versions. That is the architect's cut commit.

## 7. How this is proven (for the review)

**Before the commit:**
- all gates, locally, on the default toolchain and on 1.85;
- the cross-target check: `cargo check --workspace --all-targets --locked --target x86_64-pc-windows-gnu`
  and `--target x86_64-apple-darwin` both clean.

**After the approved commit is pushed:**
- **the CI run on all four platforms, green.** Report the run id and each job's result, and which tool
  suites (hg, SVN) ran where.
- **`release.yml` in a dry configuration:** do not push a tag. Instead:
  - run the `verify` logic and the two tools (§4) locally or through CI's own tool tests;
  - show the negative cases fail: a tag not on `main`, a version mismatch, a missing CHANGELOG heading.
    Scripted against a scratch clone is fine.
  - The first real run of `release.yml` is the architect's 0.1.1 cut, after the owner's setup.

## 8. Non-change scope

- Decoder behaviour, apart from §1 (identical output for every input on Unix; Windows gains support).
- The IR contract, and the machine output.

## 9. Security gate

- This touches the supply chain (T-4, a publish credential) and read confinement on a new platform (C-2c).
- The architect reviews it against `brygge-03` and revises the threat model to v0.4, adding:
  - the release-pipeline control;
  - the Windows confinement evidence;
  - the agent-credential residual from RFC 012 D-2.

## 10. Review request

File `.git-exclude/review-request/015-release-automation.md` with the standard sections, plus:
- the Windows junction and symlink findings (§1.2);
- the pinned action SHAs with their versions;
- the negative-case evidence (§7).
