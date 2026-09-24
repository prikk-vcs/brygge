# Changelog

All notable changes to brygge are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow the `0.y` rule of
`ROADMAP.md`: until 1.0, a minor release may make breaking changes, and they are marked **Breaking**. The
IR contract has its own version, carried in every artifact (`docs/src/reference/ir-artifact-format.md`).
Every handoff adds its own entry in its own commit.

## [Unreleased]

### Added

- **`tools/bench` measures the three remaining scale targets, and records a baseline on 0.1.1** (0.2.0 batch A;
  dev-only, not published, not run in CI). New scenarios: `git-commits` (a fixed tree, then many commits: the Git
  snapshot cache), `git-content`, `cvs-revs` (long trunk delta chains: CVS reconstruction), and `svn-dump` (a
  content-heavy dump). Each is deterministic and checks that the decode reproduces the generator's atoms, blobs,
  content bytes (and, for `cvs-revs`, every revision's text). Only the decode is timed, and its peak memory is
  measured from a reset mark. The baseline tables are in `tools/bench/README.md`. No decoder code changed.

### Fixed

- **The release workflow no longer reports success without releasing.** A dispatch with the binaries off skipped
  the check of what was published and the GitHub release, and the run still ended green; those jobs now run,
  and a final job fails any run in which the release was not made.
- **`RR-cvs-read-toctou` is closed** (0.2.0 batch C). The CVS reader examines each entry without following
  links, then opens the `,v` file by path; a link swapped in between was followed. The opened file must now be the
  file the walk saw (same device and inode on Unix; on Windows a file that is opened without following a link and
  is neither a symlink nor a reparse point), otherwise the read fails with "<path> changed while being read".
  A repository that is not changing decodes byte for byte as before.
- **A GitHub release is marked "Latest" only when it is the highest version.** Releasing an older tag (0.1.0
  after 0.1.1) had made it "Latest"; `tools/release-latest.sh` now decides, comparing versions as numbers, and
  the workflow passes its answer to `gh release create --latest`.

### Changed

- **Less memory for long Git histories** (0.2.0, RFC 010 increment 5). The Git decoder no longer keeps a snapshot of
  every commit's tree: it drops each once the last commit that needs it has been processed. Decoding 20,000 commits
  over a 500-file tree used 1.16 GiB and now uses about 99 MiB (12 times less); the peak per commit fell from about 59
  KiB to about 5 KiB. The output is unchanged, byte for byte.
- **The CVS drop reason no longer names a version** (0.2.0 batch C). In a CVS artifact with branch revisions, the
  reason text of the dropped-branch records is now "brygge imports the CVS main line only; branch history is
  planned for a later release (0.3.0). Keep the source repository." (it began "brygge 0.1.0 imports ..."). Only
  that text changes; nothing else in any artifact does.
- **CI caches the pinned mdBook binary** (0.2.0 batch C), so the book check and the deploy no longer compile
  mdBook on every run. Changing the pinned version in `tools/install-mdbook.sh` invalidates the cache.
- **A release no longer waits for a manual approval in GitHub.** The owner's go-ahead, given before the
  architect pushes the tag, is the authorization, and the tag starts the publication. The `release`
  environment stays, without a reviewer: it still limits publishing to `main` and release tags, and it
  scopes crates.io trusted publishing. (0.1.1 was released with the approval gate, which then proved too
  costly per release for v0.)

## [0.1.1] — 2026-09-24

**Windows support, and releases cut by an owner-approved workflow.** brygge 0.1.0 could not be built on
Windows; 0.1.1 builds and is tested there, as on Linux (x86_64 and arm64) and macOS (Apple Silicon). From
0.1.1 on, every release ships prebuilt binaries for those four platforms, with checksums and
build-provenance attestations. Decoding is unchanged on every platform 0.1.0 supported: the same input
gives the same artifact, apart from the brygge and decoder versions its provenance records. Install with
`cargo install --locked brygge`, or download a binary from the GitHub release.

The detailed changes that made up 0.1.1 follow.

### Windows support, CI on every platform, and an owner-approved release workflow (RFC 012)

#### Fixed

- **brygge now builds and runs on Windows.** 0.1.0 could not be built there: the CVS decoder used a Unix-only
  path API. Paths are now read through `OsStr::as_encoded_bytes`, so a Windows name that is not valid Unicode
  (an unpaired surrogate) is refused as `non-utf8-path`, exactly as a non-UTF-8 Unix name is. Nothing changes on
  Unix: every input decodes to the same bytes as before.

#### Added

- **CI proves every supported platform:** fmt, clippy `-D warnings` and test run on Linux x86_64, Linux arm64,
  macOS (Apple Silicon) and Windows, on stable; the MSRV (1.85) run, the supply-chain checks and the link
  check stay on Linux. The Mercurial and Subversion suites run where `hg` and `svnadmin` are installed (Linux
  x86_64) and skip elsewhere. Tests that need the Unix file API are `cfg(unix)`; read confinement is now
  tested on Windows too (file and directory symlinks, and directory junctions, are refused in the CVS
  scanner; a redirected `.git`, `objects`, `objects/info` or `objects/pack` is refused in Git).
- **A release workflow** (`.github/workflows/release.yml`): a pushed tag `X.Y.Z` is verified (annotated, on
  `main`, equal to the workspace version, one CHANGELOG heading), gated by the same jobs as CI, built for four
  platforms, and then, on the owner's approval of the `release` environment in GitHub, published to crates.io
  by trusted publishing (no stored secret), installed and smoke-tested from crates.io, compared byte for
  byte with the tag, and released on GitHub with checksums and build-provenance attestations. It is safe to
  re-run. The procedure and the one-time setup are in `docs/src/development/releasing.md`.
- **Release tools** in `tools/`, each with tests (`tools/test-release-tools.sh`, run by CI):
  `release-notes.sh`, `check-release-tag.sh`, `check-release-absent.sh`, `crate-published.sh`,
  `publish-crates.sh`, `check-published.sh` and `smoke-test.sh`. `check-published.sh` and
  `release-notes.sh` also serve the manual fallback.
- **A weekly `cargo audit`** against a fresh advisory database (`security-audit.yml`).
- A `.gitattributes`: test data is never line-ending converted; shell scripts and Rust sources keep LF.
- **The documentation is published as a book at <https://prikk-vcs.github.io/brygge/>, built with mdBook from `docs/src/` on every change to `main`.**
  CI builds the same book, with the same pinned mdBook, on every branch push and pull request, and a warning
  fails the build (`tools/build-book.sh`). `tools/check-links.sh` also refuses a relative link from a page of
  the book to a file outside `docs/src/`, which would be a 404 on the site.

#### Changed

- CI's actions are on their current majors (`actions/checkout` v7, off Node.js 20), and its workflow
  permissions are `contents: read`. Nothing in CI depended on `ubuntu-latest` being Ubuntu 24.
- CI runs on branch pushes and pull requests only; a release tag is gated by the run the release workflow
  calls, not by a second, duplicate run. Test steps run with `--no-fail-fast`.
- The x86_64 Linux binary is built on `ubuntu-24.04`, not `ubuntu-latest`, so a runner-image change cannot
  silently raise the glibc version it needs.
- Two tests that need a file name that is not valid UTF-8 do not run on macOS, whose file systems (APFS,
  HFS+) cannot hold such a name; the escaping they check stays tested on every platform.
- Released tags are protected by a governance rule, not a GitHub ruleset.

## [0.1.0] — 2026-09-24

**The first release.** brygge carries version-control history out of Git, Mercurial, Subversion and CVS
into an intermediate representation (IR) that belongs to no system. The IR is ready for a target's
importer, and it is honest about what the source stated, what brygge derived, and what was not carried.

- **Three commands:**
  - `decode <git|hg|svn|cvs> <source> --out <artifact>`;
  - `inspect <artifact>`;
  - `verify <artifact> [--against-source <source>]`.

  Each has human and versioned machine output (`docs/src/reference/machine-output.md`), and exit codes
  that carry the outcome class.
- **IR contract 0.2.0,** a published, strict, integrity-checked binary format that can evolve safely
  (`docs/src/reference/ir-artifact-format.md`).
- **Faithful by construction:**
  - text is carried as bytes;
  - only claims the source stated are made;
  - source ids are verified against their content;
  - derived results (inferred renames, CVS changesets, SVN branches) are marked as brygge's judgment;
  - everything not carried is recorded, or refused with a named reason.
- **Per source** (user guides in `docs/src/guide/`):
  - **Git** (pure Rust): full history of branches and tags; signatures, extra headers and annotated tags
    carried.
  - **Mercurial** (pure Rust): the published view, the same changesets `hg clone` shares; recorded renames
    and copies with their true source; extras.
  - **Subversion:** from a dumpfile, or via `svnadmin dump`; branches and tags reconstructed on request.
  - **CVS:** the main line, reconstructed into changesets with a recorded confidence.
- **Not in 0.1.0:**
  - CVS branch history, SVN delta dumps and Mercurial hashed long paths (0.3.0);
  - SHA-256 Git repositories;
  - streaming for very large repositories (memory is bounded by ceilings, not streamed; 0.2.0);
  - progress and cancellation;
  - the prikk encoder (it waits on prikk's import foundations).
- **Install:** `cargo install --locked brygge` (Rust 1.85+). `svnadmin` is needed only for decoding a live
  SVN repository.
- **Security model:** `docs/src/brygge-03-threat-model-v0.1.md` (v0.3).

The detailed changes that made up 0.1.0 follow, newest first, grouped by the commit that made them.
Before 0.1.0 nothing was released, so every **Breaking** mark below is relative to the unreleased
development state, not to an earlier release.


### 0.1.0 release preparation, part 2: a green CI on the MSRV, two honesty gaps, a clean machine output

#### Changed

- **Breaking (machine output):** keys are dot-separated `snake_case` segments and values are `kebab-case`, so
  keys that embed a label change (`derived.inferred-rename` → `derived.inferred_rename`, and every
  `dropped.*`/`flagged.*`; `verify.check.source-invariants` → `verify.check.source_invariants`,
  `verify.check.loss-boundary` → `verify.check.loss_boundary`). `atoms` is printed once. `atom.N.message` is
  the raw bytes percent-encoded once (a consumer can decode non-UTF-8 exactly). A check's explanation is
  `verify.check.<name>.detail`, the source comparison's is `verify.against_source.detail`, and its note is
  `verify.against_source.note`. A check that could not run is `not-checked` (was `n/a`). Fields skipped from a
  newer contract are visible as `skipped_non_critical_fields` (and `verify.skipped_non_critical_fields`). An
  `Other(name)` status or kind prints the label `other` with a companion `…_other` key. Versions:
  `report_version` 3, `inspect_version` 4, `verify_version` 4. Exit codes are unchanged.
- **Breaking:** `--infer-renames` with `hg` is now a usage error (exit 2): Mercurial records its renames, so
  nothing is inferred, and hg artifacts no longer record `infer_renames`, `rename_algorithm` or
  `rename_threshold` in their params. `--help` says `--infer-renames (git)`.
- Subversion: a non-UTF-8 `svn:author` or `svn:log` is now carried byte-exact instead of being dropped without
  a record.
- Mercurial: the obsolescence-marker file is read through a bounded reader (no check-then-read), and
  `.hg/bookmarks` is parsed once, strictly and bounded, instead of twice.
- The CI build on the declared MSRV (1.85) works again: `Cargo.lock` holds the transitive `human_format` at
  1.1.0 (1.2.1 does not build on 1.85). Lock-only; no new crate. Six test-code clippy findings that only 1.85
  reports are fixed.

#### Added

- The local gate list gains the MSRV run (`cargo +1.85 clippy` and `cargo +1.85 test`), and every report
  states CI's result for the pushed commit.

### 0.1.0 release preparation: verified Mercurial nodes, one floor vocabulary, plain-language text

#### Changed

- **Breaking:** floor features are now identifiers (lowercase kebab-case), and the same concept has the same
  identifier in every decoder (`non-utf8-path` in Git, hg and CVS; `remote-source` in SVN and CVS). Refusals
  and `params["floor"]` use them, so **`params["floor"]` changes in every Git, hg and SVN artifact**.
  Renamed: Git `replace-ref`, `shallow-clone`, `object-alternates`, `redirected-git-directory`,
  `non-utf8-ref-name`, `non-utf8-commit-header-name`, `sha256-object-format`; hg `censored-revision`,
  `unfinished-merge`, `non-utf8-extra-key`; SVN `svn-externals`. Each decoder's README has a Floor table.
- **Breaking:** artifact text (drop and flag reasons, refusal messages) and `--help` no longer carry internal
  RFC/requirement numbers or stale phrases, so the reasons in artifacts change. `--help`'s vocabulary now
  lists **flagged**.
- **Breaking (repositories with a secret root):** the Mercurial `repo_id` is now the smallest root node among
  the *published* changesets, not among all changesets, so it equals the identity of an `hg clone` of the
  repository and every identifier that reaches the artifact comes from a changeset that was read and verified.
  A repository whose smallest-node root is secret gets a different `repo_id`; other repositories are unchanged.
- Mercurial: a non-UTF-8 manifest path is now a refusal (exit 20, bytes shown as `\xNN`) instead of a read
  error, matching Git and CVS. Subversion keeps a read error (a dumpstream's paths are UTF-8 by definition)
  and now shows the bytes as `\xNN`.
- The output artifact's temporary file is created exclusively: a file or symlink already at that path makes
  the write fail and is never followed, truncated or removed.

#### Added

- Mercurial: every revision brygge reads (changelog, manifest, filelog) is checked against its node with
  Mercurial's own revision hash (collision-detecting SHA-1), so a crafted store cannot present content under
  a node it does not hash to. Revisions whose stored text is not the hashed text (ellipsis, external
  storage) and unknown revision flags are refused by name. Adds a dependency edge to `sha1-checked`, already
  in the lock file through `gix`; no new crate.
- CI runs the documentation link check.

### Hardening: overflow checks stay on in release builds

#### Changed

- `[profile.release]` sets `overflow-checks = true`. A wrap of an integer that reaches a claim (found in
  the CVS date parser during review) would otherwise be silent in release builds; with checks on, any
  such wrap becomes a panic, which the decoder panic boundary turns into a typed failure (exit 1), never
  a false value.

### IR contract re-cut to 0.2.0

#### Changed

- **Breaking:** the IR contract is re-cut from the frozen 1.0.0 to **`0.2.0`** (RFC 011): tagged records
  with a critical bit replace the positional codec, a strict canonical form is enforced on read, and the
  integrity digest covers the stored bytes directly (never a re-encoding). **Pre-release artifacts must
  be re-decoded** — a format-1 artifact is refused with a clear message (`Error::PreReleaseArtifact`),
  its metadata never parsed.
- The model gains `Text` (byte-exact, not assumed UTF-8) and `Time` (with an optional source UTC
  offset); `RenameHint` is renamed `CopyRecord` and gains `from_atom`, so a copy names the exact earlier
  atom it came from; `PathOp` gains `Replace`; `Flag`/`FlagKind` replace the magic-string exit-30
  detection (`LAYOUT_UNMATCHED`/`UNDER_FLOOR`, removed); `Signature` and `Extra` carry opaque
  source-specific data by label; `RefRecord` gains an optional `Annotation`. `ImportProvenance` loses
  `import_time`.
- `from_bytes` now returns `Decoded { ir, skipped_non_critical_fields }`, surfacing how many fields from
  a newer contract minor were skipped (always non-critical, so never able to change what the artifact
  claims).
- The fidelity report is v2 (`REPORT_VERSION = 2`): `refused` is replaced with `flagged`, populated from
  `ir.flags`.
- SVN copies now resolve the **correct** source revision as `from_atom` (previously not representable);
  Git/hg/CVS carry message and identity text as raw bytes (no lossy UTF-8 conversion).
- The CLI: `verify`'s verdict is three-valued (`pass`/`fail`/`incomplete` — a requested
  `--against-source` that could not be checked no longer reports `PASS` while exiting non-zero);
  `inspect --atoms` shows copies (marking moves), flags, signatures and extras; machine formats bump to
  `inspect_version=3`/`verify_version=3`.

#### Added

- The published IR wire-format reference, `docs/src/reference/ir-artifact-format.md` (`PU-3`): every
  primitive, canonical rule, the container layout and read order, every record's field table, and an
  annotated, byte-verified worked example.
- A required-test suite in `brygge-ir`: round-trip coverage for every record/enum/variant; one test per
  canonical rule in RFC 011 §2.2/§2.5; unknown-field handling (critical and non-critical); the version
  gate (pre-release, wrong minor, accepted patch bump); digest tamper detection per container section; an
  `AtomId` compatibility vector; and a panic-freedom sweep over every truncation and single-byte mutation
  of a valid artifact.

### CVS decoder corrections: main line only, honest clustering, content-derived identity

#### Changed

- **Breaking (artifact contents change):** brygge now imports a CVS repository's **main line only**
  (owner ruling D-2): the trunk, plus — while a vendor branch is set — that branch's own revisions (what
  a plain `cvs checkout` actually yields). Every other revision is a branch revision: excluded, and
  recorded in a single drop record with its count, never silently landing in a replayed main-line tree
  (CR-01). Branch symbols (tags naming a branch, not a revision) are likewise not reconstructed, and are
  counted in their own drop record — including a **vendor** branch's own symbol, which RCS stores as a
  literal odd-length number rather than the usual magic form, and which now correctly counts as a branch
  rather than an unresolvable tag (review 008 R-2). A **tag** that names no main-line revision at all
  (common once main-line-only import excludes what it pointed at) is counted in its own drop record too,
  never silently skipped (review 008 R-1). See `docs/src/guide/cvs.md` for what this means for a
  migration.
- **Clustering is now deterministic and trustworthy (CR-08.2):** revisions are grouped by `(author,
  log)` first, then split by time window and by at-most-one-revision-per-path — replacing the old
  sequential greedy pass, which could fragment an interleaved author's own commits. Confidence now also
  accounts for how much a cluster's paths overlap other nearby clusters (rule `span-overlap-v1`), not
  just its own time span; all of its date/window arithmetic saturates rather than wrapping on an extreme
  input (review 008 R-3). A rare inter-cluster date-skew artifact that would place a later-numbered
  revision before an earlier one is repaired by splitting it into its own changeset, counted
  (`order_splits`). Every `ReconstructedChangeset` derivation now carries `confidence_rule` and
  `date_rule`, both of which `verify`'s `derivations` check requires from this point on (review 008 R-8).
- **`repo_id` is now a content-derived fingerprint (CR-08.1)**, not the operator's filesystem path: a
  SHA-256 over every file's lowest-numbered trunk revision (path, revision, date, author), in path order
  — not necessarily `1.1`, since a file need not start there (review 008 R-6). The same repository
  decoded from two different locations now gives byte-identical artifacts. A repository with no trunk
  revision anywhere is refused (`Error::Read("no main-line revisions")`) rather than fingerprinted as an
  empty hash.
- **No committer or commit time the source never stated (CR-04):** only `author`/`author_time` are
  carried (the one-claim rule); `author_time.offset_minutes` is always `Some(0)` (RCS dates are UTC). An
  unparseable date, or a year outside `1970..=9999`, is now a read error rather than a fabricated `1970`
  claim (review 008 R-3). The author login is now carried as the source's exact bytes, never converted
  lossily (review 008 R-4).
- **Repository-shape refusals added (CR-08.3/8.4/CR-03):** the same path present in both `Attic/` and
  live, a symlink anywhere under the repository root, and a non-UTF-8 path component (shown as `\xNN`,
  never converted lossily) are now refused rather than silently mishandled. Two more join them: a
  non-UTF-8 symbol name (review 008 R-4), and a file whose default branch is set but which also has
  trunk revisions after that branch's branch point — an ambiguous main line brygge refuses to guess
  (review 008 R-5) — as does a present-but-unparseable `branch` admin field, now a read error instead of
  a silent "no vendor branch".
- The faithfulness statement printed before every CVS run now states the main-line-only limitation.

### Mercurial decoder corrections: the published view, true copy sources, claims and extras

#### Changed

- **Breaking (artifact contents change):** brygge now imports a Mercurial repository's **published
  view** (owner ruling D-4, revised) — exactly what `hg clone` would transfer. Secret/archived/internal
  changesets (phase `>= 2`) are excluded; obsolete (rewritten or pruned) changesets are excluded unless
  they are an ancestor of something non-obsolete or pinned (a bookmark target, a working-directory
  parent, or a local tag — `.hgtags` does **not** pin, correcting the handoff's first draft). Both
  exclusions are counted in the loss boundary, never silent. A repository with an unresolved merge in
  progress is refused (floor feature `unfinished merge`) rather than guessed at.
- A stated copy's `from_atom` now names its **true** source changeset (RFC 011 D-6), resolved by: p1,
  else p2, else the copy's own filelog linkrev (if published and an ancestor), else a bounded
  first-parent ancestry walk — never placed on a guess. A copy that cannot be placed this way is omitted
  and counted (`copy sources not resolvable (N)`); a copy stated on a path that already existed (a
  modify, not an add) is counted separately, since the IR's copy model has no home for it.
- **One-claim rule:** `committer`/`commit_time` are no longer copied from `user`/`date` — they are always
  absent (hg states only one identity/time per changeset).
- The timezone offset now converts correctly (`offset_minutes = -(tz / 60)`, hg's seconds-west
  convention); an offset that does not divide evenly into minutes is counted
  (`unrepresentable timezone offsets (N)`) rather than silently dropped.
- Every changelog extra except `branch` is now carried as a labelled `Extra` (notably `close`, meaning a
  closed branch), decoded byte-exact per Mercurial's own escaping — but stricter: an escape, a duplicate
  key, or an entry with no `:` that `hg` itself could never have written is refused, not guessed. A
  non-UTF-8 extras key is a floor refusal.
- Bookmarks naming an unimported changeset are now counted, never skipped silently.
- The obsstore reader's `usingsha256` flag bit is corrected to `2` (was `1 << 8`), matching
  `obsutil.py`'s definition (`bumpedfix = 1`, `usingsha256 = 2`).

### SVN decoder corrections: symlinks, replacements, live refs, layout honesty, source form

#### Changed

- **Breaking (artifact contents change):**
  - **`PathOp::Replace` is now produced** (CR-07.2, RFC 011 D-7): an `svn rm`+`svn add` (or a `replace`
    node) at one path in one revision is carried as a stated replacement, even when the content is
    identical, instead of being folded into `Modify`.
  - **Reconstructed refs now name only live history** (CR-07.3): with `--reconstruct-refs`, a branch or
    tag root is emitted only if at least one file still lives under it in the final tree. A deleted or
    moved-away root is not emitted; it is counted (`deleted or moved branches/tags not represented (N)`).
    A moved root appears only under its new name.
  - **A repository that only partly follows the trunk/branches/tags convention is now flagged**
    (CR-07.5): paths outside every recognized root are counted and raise a `ConventionViolation` `Flag`
    (CLI exit 30) — but only when the layout was found at all; a flat repository still raises just the
    one whole-layout-not-found flag, never both.
  - **The source form is now recorded**: `source_form` (`dumpfile` or `svnadmin-dump`) and, for a live
    decode, `svnadmin_version`, both in provenance params. `verify --against-source` reports
    `not-checked` (never a false mismatch) when the given source's form differs from what was recorded,
    and ignores an `svnadmin` version difference alone when comparing two `svnadmin-dump` decodes.
  - **No committer or commit time the source never stated (CR-04):** only `author`/`author_time` are
    carried (the one-claim rule); `author_time.offset_minutes` is always `Some(0)` (`svn:date` is UTC).
    An unparseable `svn:date` is counted, never guessed.
- **Fixed a content-corruption bug (CR-07.1):** a symlink retargeted in a revision with no property block
  of its own (properties inherited implicitly, as SVN itself behaves) previously stored the raw
  `link <target>` text as file content instead of the bare target.
- **Fixed an honesty gap (CR-07.4):** every added *or replaced* directory that ends up with no file under
  it after the revision is now counted as an empty directory; before, only `add`-kind directories were
  checked, so a replaced empty directory could be silently dropped with no record.

### Git decoder corrections batch 2: verified history, verified tag chains, encodings, timezones, extras, annotations

#### Changed

- **Breaking (a real integrity gap closed):** history is now walked with the on-disk `commit-graph`
  cache disabled (`.use_commit_graph(false)`) and every commit's parents are read from its own verified
  content; a verified commit whose parent the walk did not otherwise reach is refused
  (`Error::Read("history walk disagrees with commit content")`) rather than silently treated as a root.
  A crafted `objects/info/commit-graph` could previously drop or fabricate a parent — truncating or
  altering the imported history — even though every individual object was already re-hashed against its
  id. `RR-git-object-id-unverified` and `RR-git-loose-object-symlink` close.
- **Breaking:** a tag of a tag is now peeled and verified all the way to its final target,
  bounded by a new `max_tag_chain` ceiling (default 32). Every intermediate tag object is counted
  (`nested tag objects not carried (N)`) rather than silently followed without verification.
- **Breaking:** `params["floor"]` gains `SHA-256 object format`, refusing (rather than misreading) a
  repository using `extensions.objectFormat = sha256`, which this build's `gix` dependency cannot read
  or verify (only `sha1` is enabled).
- **Breaking:** annotated tags are now carried (`RefRecord.annotation`: tagger, time, message), and the
  old "annotated tag tagger and message" drop record is gone — a repository whose only recorded loss was
  annotated tags now exits `0`. `gpgsig`/`gpgsig-sha256` are carried as labelled `Signature`s; every
  other commit header (including `mergetag`, unfolded byte-exact) is carried as a labelled `Extra`.
- **Breaking:** the commit `encoding` header, when present and valid UTF-8, is carried on the message's
  `Text.encoding` only (never also as an `Extra`); an undecodable `encoding` value is counted
  (`undecodable encoding headers (N)`), never silently dropped or misapplied.
- **Breaking:** author/committer/tagger timezone offsets are parsed strictly (`+HHMM`/`-HHMM`, `MM < 60`)
  — never gix's lenient parser, which silently defaults to `+0000`. A malformed offset is absent and
  counted (`unparseable author/committer/tagger timezone offsets (N)`), never fabricated; the same
  applies to a tagger's own time now, not just the author's and committer's.
- A non-UTF-8 commit header **name** is a new floor refusal, `non-UTF-8 commit header name` — labels are
  text.
- The "commits reachable only from dropped refs (count unavailable)" record's reason is now the fixed
  text `"a commit in dropped-only history could not be read"`; the decoder no longer writes to the
  process's stderr (it was the only decoder in the workspace that did, bypassing the CLI's
  neutralization).

### `97b0b03` — Project hygiene: enforced isolation, declared floors, narrow APIs, accurate docs

#### Added

- `tools/check-ir-isolation.sh` and `crates/brygge-ir/allowed-dependencies.txt`: `brygge-ir`'s dependency
  isolation is now a CI-enforced, tested property, not merely an asserted one (RFC 009 D-7, CR-12.1).
- Each decoder declares its floor (the list of refused features) in one private `floor` module, matching
  what its refusals actually use, and records it in every artifact's provenance as `params["floor"]`, so a
  reviewer reads which floor applied from the artifact itself (CR-12.2, PR-5). RFCs 004–007 amended with
  one sentence recording this.
- `CHANGELOG.md` (this file).

#### Changed

- Owner-authorized the 0.1.0 correction-cycle release plan, after the architect's intake review found
  defects in correctness, honesty and contract evolution that must be fixed before a first release
  (`420b88d`).
- Each decoder crate's public API is narrowed to what a caller needs: `decode`, `Options`, `Error`,
  `decoder_version` (all four); `Source` (SVN, CVS); `LayoutPolicy` (SVN) (CR-20). Mercurial's `revlog`
  and `requires` modules, CVS's `rcsfile` re-export, and SVN's `dump` module are now crate-private; each
  crate's `DECODER` constant is no longer public, matching Git's and Mercurial's existing convention.
- Documentation accuracy pass (CR-14): fixed two relative links left broken by the `HANDOFF.md`/
  `GOVERNANCE.md` move (`74dc0eb`); rewrote the Mercurial decoder's README and crate doc (no longer
  "foundation increment" — it is built); replaced `rfcs/README.md`'s long state prose with a
  state-grouped index table (Accepted · Done · Proposed · Archive) per RFC 000's own recommendation;
  corrected `GOVERNANCE.md`'s C-4b description (a C surface may also be isolated to a subprocess, not
  only a dedicated FFI crate, threat model v0.2); corrected `HANDOFF.md`'s delivery-status wording
  ("built, not yet released", not "delivered") and its stale threat-model-residual list (`RR-svn-svnadmin`
  and `RR-svn-svnadmin-version` were already folded into `brygge-03` v0.2; only `RR-cvs-reconstruction`
  remains outstanding).

### `8afc316` — Input ceilings that actually bound, and the hg format gate

#### Added

- Resource ceilings — blob/dumpstream/RCS-file size, commit count, path length, tree depth, and
  decompressed-revision size — are now checked **before** the memory they protect is allocated, in all
  four decoders; a hit is a typed refusal (CLI exit 20) with a unit-bearing message
  (`refused: <what> exceeds brygge's ceiling (<value> <unit>)`), never an OOM, a stack overflow, or a hang
  (RFC 010 D-4, CR-10/CR-17).

#### Changed

- Git: `walk_tree` is now iterative (an explicit stack) instead of recursive, so an attacker-chosen tree
  depth can no longer overflow the process stack — the one path CR-16's panic boundary could not help.
- Mercurial: the format gate now distinguishes a missing `.hg/requires` file from any other read error (a
  permission bit no longer silently disables the gate); a path needing the hashed `dh/` store encoding and
  non-UTF-8 changeset metadata are now refusals (CLI exit 20) rather than read failures (exit 1) (CR-15).
- **Mercurial `repo_id` is now the smallest root changeset node (Git parity), not revision 0.**
  Mercurial numbers revisions by local pull order, not content, so two clones of a repository with more
  than one history root, pulled in different orders, previously got different `repo_id`s; they now match.
  This only changes output for a multi-root repository (rare); a single-root repository's `repo_id` is
  unaffected, since revision 0 was already the unique root and the minimum.
- Subversion: `svnadmin dump`'s stdout and stderr are now drained concurrently on separate threads, so a
  full pipe on either stream cannot deadlock or starve the child process.

#### Fixed

- Mercurial: revlog decompression (zlib and zstd) and `mpatch` delta application are now bounded against a
  decompression bomb (a small stored chunk that inflates to an enormous text); every reconstructed
  revision's length is independently checked against its index entry, a mismatch being a malformed-store
  error, never a ceiling (CR-17).

### `416a6ca` — Git decoder corrections batch 1: history scope, repository shape, path integrity

#### Fixed

- Git: history import is now scoped to carried refs only (non-symbolic `refs/heads/*` and `refs/tags/*`);
  a commit reachable only from a dropped namespace (`refs/remotes/*`, `refs/notes/*`, `refs/stash`, …) is
  never imported (CR-05).
- Git: a repository using object alternates, a redirected git directory (a `gitdir:` file or a
  `commondir`), or containing a symlinked `.git` entry, `objects`, `objects/info`, `objects/pack`, or pack
  file is now refused rather than silently read from outside the given repository (CR-11, owner ruling
  D-3(ii)).
- Git: a non-UTF-8 path or ref name is now refused — with every invalid byte escaped as `\xNN`, never
  substituted — rather than converted lossily (CR-03, owner ruling D-3(i)).
- Git: a `refs/heads/*` or `refs/tags/*` ref that peels to a tree or blob (not a commit) is now recorded
  as a drop rather than silently skipped; a ref whose target is missing entirely is now a hard read error,
  a broken-repository signal (CR-06).

### `e087651` — 0.1.0 batch 1: the three-verb CLI surface and honest verification

#### Changed

- **Breaking: the CLI surface is replaced** with three verbs — `decode`, `inspect`, `verify` — and
  positional-noun arguments; the previous invocation forms are now usage errors (exit 2).
- `verify --against-source` now reports a third outcome, `not-checked`, for a source that could not be
  re-decoded (a wrong path, an unreadable source, a refusal) or has no decoder — previously reported as a
  mismatch (`does not correspond`), which read as possible tampering when nothing was actually compared.

#### Added

- Every `verify` check that can actually fail: structure, replay, derivations, source-invariants,
  provenance, and the loss boundary (internal), reported separately from `--against-source` correspondence
  — the two claims are never merged.
- Atomic artifact writes: a failing `decode` leaves an existing artifact untouched and no temporary file
  behind.
- Output neutralization (CR-19): every string that can originate in a source repository is routed through
  a fixed escaping layer before it reaches stdout or stderr, closing a terminal/log-injection surface —
  including bidi and invisible-format control points, line/paragraph separators, and ANSI escapes.
- A faithfulness statement, printed before every `decode`/`verify` run regardless of outcome (VF-5).
- Bounded replay during `verify`: peak memory and clone count are tracked directly (a reference-counted
  first-parent tree, taken rather than cloned at the last reference), not inferred from RSS.

### `20f43f9` — RFC 004: fix CR-16, `decode git` panics on symbolic refs

#### Fixed

- Git: `decode git` no longer panics on a repository carrying a symbolic ref under `refs/` — the shape
  every ordinary `git clone` creates (`refs/remotes/origin/HEAD`). The symbolic ref contributes no walk
  tip and is not carried as a ref (the IR has no alias concept); its target, if any, is a distinct ref
  carried or dropped on its own merits.
- An unparseable author/committer time now becomes an absent claim, never fabricated as `0` or salvaged
  from a malformed token (NG-5).

#### Added

- A panic boundary around every decoder invocation (`guard_decoder`): an unexpected panic inside a decoder
  or a decoder dependency is now a typed runtime failure (exit 1), never an unclassified process abort.
