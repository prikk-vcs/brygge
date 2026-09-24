# brygge

**brygge** (Norwegian: *wharf* — where cargo is landed) carries version-control history **out of** an
existing system (Git, Mercurial, Subversion, or CVS) into an **intermediate representation (IR)** that
belongs to no particular system, so a target — prikk first — can encode from it. The guiding idea is
**faithfulness with provenance, not neutrality**: brygge records what the source *literally guaranteed*,
and marks — in the object itself — anything it had to *infer*. It reads untrusted repositories, links no
network, and writes only where you tell it.

## Install

```sh
cargo install --locked brygge
```

brygge needs Rust 1.85 or later to build. `--locked` builds the exact dependency set brygge was
tested with; without it, cargo may pick newer dependencies that need a newer Rust. Decoding a live
Subversion repository also needs `svnadmin` on your `PATH`; decoding a dumpfile needs nothing else.
Prebuilt binaries for Linux (x86_64, arm64), macOS (Apple Silicon) and Windows (x86_64) are attached to every
[GitHub release](https://github.com/prikk-vcs/brygge/releases) from 0.1.1 on, with checksums and build-provenance
attestations; `docs/src/development/releasing.md` shows how to check one.

## Vocabulary

**artifact** the IR file `decode` writes · **source** the repository or dumpfile read · **stated** the
source's own record · **derived** brygge's judgment, marked as such · **dropped** not carried, and
recorded · **flagged** carried, but recorded as needing your attention · **refused** the whole import
declined, so no artifact is written · **Unverifiable** imported
authorship cannot be checked — a property of the claim, not a pending check.

## 30-second start

```sh
# Decode a source repository into an IR artifact (--out is required):
brygge decode git  /path/to/repo            --out out.ir     # or hg | svn | cvs
brygge decode svn  /path/to/repo-or-dumpfile --out out.ir --reconstruct-refs
brygge decode cvs  /path/to/cvsroot/module   --out out.ir

# Read what you got:
brygge inspect out.ir            # the fidelity report (below)
brygge inspect out.ir --atoms    # + atoms, their status, source ids, the loss boundary

# Check it:
brygge verify out.ir                                   # the honesty checks; no source needed
brygge verify out.ir --against-source /path/to/repo     # + re-derive and compare
```

Add `--format machine` to any of these for stable, line-oriented output a script or CI can read.

## Reading the fidelity report

This is the one skill worth learning, because it is how you know a migration is safe. Every `decode` and
`inspect` prints it:

```
fidelity report (v3) — what brygge imported, and how much is the source's own record
vs brygge's judgment. Authorship is Unverifiable (carried as the source claimed it; no target can verify it).

  preserved: 128 atom(s), 3 ref(s), 512 blob(s), 1048576 content byte(s)
  derived:   brygge's judgment, not the source's fact — a later brygge version could differ; ...
    reconstructed-changeset: 128
  not history (no content or claim lost): 3
  dropped (recorded loss):
    advisory-unreliable: 2
  flagged:   recorded, and why this import needs your attention:
    below-confidence-floor: 4
```

- **preserved** — carried faithfully, as the source recorded it.
- **derived** — brygge's *judgment*, not the source's fact (an inferred rename, a reconstructed CVS
  changeset, a convention-guessed SVN branch). A different brygge version might judge differently, so it is
  marked *in the object* — `inspect --atoms` shows exactly where. **If this section is non-empty, part of
  your import is brygge's interpretation, and you should know which part.**
- **not history** — ordinary storage layout brygge doesn't carry (packfiles, the working copy, reflogs):
  nothing was lost, so this is one line, not a list to worry about.
- **dropped (recorded loss)** — things not carried, but *recorded here*, never silently lost (advisory data
  like SVN `svn:mergeinfo`; working-copy transforms like keyword expansion).
- **flagged** — carried, but recorded as needing your attention (an SVN layout brygge could not follow,
  CVS changesets below the confidence floor). A flagged import exits `30`.
- **Authorship is always `Unverifiable`.** brygge faithfully carries who the source *claimed* authored
  something; it never asserts that claim was verified by anyone.

## What each source can and cannot promise (before you run)

Faithfulness means something different per source, so brygge tells you up front, before decoding starts.
Each source has a user guide with the details, including what it refuses and what you can do about it:

- **Git** ([guide](docs/src/guide/git.md)) — a real history graph from branches and tags, every object
  verified against its id. Renames are *inferred* only with `--infer-renames`, and are then marked
  `derived`. Submodules, replace refs, grafts and shallow clones are among the shapes refused.
- **Mercurial** ([guide](docs/src/guide/hg.md)) — what the repository would publish (the changesets
  `hg clone` shares), every revision verified against its node. Renames and copies the source recorded
  come through as *stated*, not guessed. Subrepositories, largefiles, lfs and censored revisions are among
  the shapes refused.
- **Subversion** ([guide](docs/src/guide/svn.md)) — atomic revisions import as a stated spine. **Branches and
  tags are directory *convention***, so reconstructing them (opt-in, `--reconstruct-refs`) is `derived`.
  `svn:externals` is refused; `svn:mergeinfo` is dropped-with-record, never a merge parent.
- **CVS** ([guide](docs/src/guide/cvs.md)) — **there is no atomic commit**, so brygge *reconstructs*
  changesets by clustering per-file revisions. **Every CVS changeset is therefore `derived`**: a labelled
  reconstruction, not the source's record. Content and per-file history are faithful; a low-confidence
  reconstruction is flagged or refused. brygge imports the **main line** only (branch history is planned for 0.3.0). For CVS,
  `verify --against-source` checks *per-file content and deterministic reproduction*, **not** changeset
  correspondence, because there is no source changeset to check against.

## Exit codes (for scripts and CI)

`0` clean · `10` recorded loss (advisory/other drops) · `20` refused: a floor feature, an unsupported
format, or a resource ceiling · `30` flagged: a convention/confidence line was crossed (an SVN layout brygge
could not follow, or a CVS reconstruction below the floor) · `50` a `verify` check failed · `1` a runtime
failure (unreadable input, I/O, an internal decoder fault), or a `verify` whose requested source comparison
could not run (`incomplete`) · `2` a usage error (bad arguments, an option given to a source kind it does
not apply to). The machine output is specified in
[`docs/src/reference/machine-output.md`](docs/src/reference/machine-output.md).

## Going deeper

- **The documentation site:** <https://prikk-vcs.github.io/brygge/>, the same pages as `docs/src/`, built as a
  searchable book.
- **Design set:** `docs/src/brygge-01-requirements-spec` (what brygge must do), `-02-external-design` (its
  surface), `-03-threat-model` (what it defends and how). brygge parses untrusted input; the threat model
  is a first-class deliverable.
- **Reference:** the IR artifact format ([`docs/src/reference/ir-artifact-format.md`](docs/src/reference/ir-artifact-format.md),
  contract 0.2.0) and the machine output ([`docs/src/reference/machine-output.md`](docs/src/reference/machine-output.md)).
- **Decisions:** `rfcs/`: one RFC per source decoder (004 Git, 005 Mercurial, 006 Subversion, 007 CVS),
  the IR foundations (001/002/003) and their re-cut (011), the dependency-surface policy (009), and bounded
  memory (010).
- **Changes:** [`CHANGELOG.md`](CHANGELOG.md).
- **Contributing / maintaining:** start at
  [`HANDOFF.md`](docs/src/development/handoffs/HANDOFF.md) (status, invariants, architecture, the
  backlog, and how to build and gate), then
  [`GOVERNANCE.md`](docs/src/development/handoffs/GOVERNANCE.md) (who decides what) and
  [`ROADMAP.md`](ROADMAP.md).
- **`encode` is not in brygge yet.** The prikk encoder (RFC 008) waits on prikk's import foundations
  (`ROADMAP.md`, Track B); this release is decode + inspect + verify.
