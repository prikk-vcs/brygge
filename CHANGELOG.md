# Changelog

All notable changes to brygge are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). brygge is in **v0 development** — no version has
been released yet; see `ROADMAP.md` for the 0.1.0 release plan. Entries are grouped by the commit that
made them, newest first, so this file is complete for everything since the architect's intake review began
— it is the source for 0.1.0's release notes. From this point on, every handoff adds its own entry in its
own commit.

## [Unreleased]

### Project hygiene (this handoff)

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
