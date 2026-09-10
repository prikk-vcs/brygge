# brygge

**brygge** (Norwegian: *wharf* — where cargo is landed) carries version-control history **out of** an
existing system (Git, Mercurial, Subversion, or CVS) into an **intermediate representation (IR)** that
belongs to no particular system, so a target — prikk first — can encode from it. The guiding idea is
**faithfulness with provenance, not neutrality**: brygge records what the source *literally guaranteed*,
and marks — in the object itself — anything it had to *infer*. It reads untrusted repositories, links no
network, and writes only where you tell it.

## 30-second start

```sh
# Decode a source repository into an IR artifact:
brygge decode git  /path/to/repo            --ir out.ir     # or hg | svn | cvs
brygge decode svn  /path/to/repo-or-dumpfile --ir out.ir --reconstruct-refs
brygge decode cvs  /path/to/cvsroot/module   --ir out.ir

# Read what you got:
brygge inspect --ir out.ir            # atoms, their status, source ids, the loss boundary
brygge summary --import out.ir        # the fidelity report (below)

# Check it:
brygge verify --internal        --import out.ir   # honesty holds, no source needed
brygge verify --against-source /path/to/repo --import out.ir   # re-derive and compare
```

Add `--format machine` to any of these for stable, line-oriented output a script or CI can read.

## Reading the fidelity report

This is the one skill worth learning, because it is how you know a migration is safe. Every `decode` and
`summary` prints it:

```
fidelity report (v1) — what brygge imported, and how much is the source's own record
vs brygge's judgment. Authorship is Unverified (imported, not verified by any target).

  preserved: 128 atom(s), 3 ref(s), 512 blob(s), 1048576 content byte(s)
  derived:   brygge's judgment, not the source's fact — a later brygge version could differ; ...
    reconstructed-changeset: 128
  dropped:   recorded here, never silently lost:
    representation: 2
  refused:   a source feature below the floor — refused rather than guessed:
    ...
```

- **preserved** — carried faithfully, as the source recorded it.
- **derived** — brygge's *judgment*, not the source's fact (an inferred rename, a reconstructed CVS
  changeset, a convention-guessed SVN branch). A different brygge version might judge differently, so it is
  marked *in the object* — `inspect` shows exactly where. **If this section is non-empty, part of your
  import is brygge's interpretation, and you should know which part.**
- **dropped** — things not carried, but *recorded here*, never silently lost (physical storage layout;
  advisory data like SVN `svn:mergeinfo`; working-copy transforms like keyword expansion).
- **refused** — a source feature brygge will not approximate, refused with a named reason rather than
  guessed at.
- **Authorship is always `Unverified`.** brygge faithfully carries who the source *claimed* authored
  something; it never asserts that claim was verified by anyone.

## What each source can and cannot promise (before you run)

Faithfulness means something different per source, so brygge tells you up front:

- **Git** — a real history graph; renames are *inferred* (off by default, and marked `derived` when on).
  Submodules/octopus/grafts/shallow are refused.
- **Mercurial** — like Git, plus **renames the source recorded** (`hg mv`) come through as *stated*, not
  guessed. Subrepos, largefiles, and censored revisions are refused.
- **Subversion** — atomic revisions import as a stated spine; **branches/tags are directory *convention***,
  so reconstructing them (opt-in, `--reconstruct-refs`) is `derived`. `svn:externals` is refused;
  `svn:mergeinfo` is dropped-with-record, never a merge parent.
- **CVS** — **there is no atomic commit**, so brygge *reconstructs* changesets by clustering per-file
  revisions. **Every CVS changeset is therefore `derived`** — a labelled reconstruction, not the source's
  record. Content and per-file history are faithful; a low-confidence reconstruction is flagged or refused.
  For CVS, `verify --against-source` checks *per-file content and deterministic reproduction* — **not**
  changeset correspondence, because there is no source changeset to check against.

## Exit codes (for scripts and CI)

`0` clean · `10` recorded loss (advisory/other drops) · `20` a feature below the floor was refused ·
`30` a convention/confidence line was crossed (an SVN layout not found, or a CVS reconstruction below the
floor) · `50` a `verify` check failed · `1` bad arguments or unreadable input.

## Going deeper

- **Design set:** `docs/src/brygge-01-requirements-spec` (what brygge must do), `-02-external-design` (its
  surface), `-03-threat-model` (what it defends and how). brygge parses untrusted input; the threat model
  is a first-class deliverable.
- **Decisions:** `rfcs/` — one RFC per source decoder (004 Git, 005 Mercurial, 006 Subversion, 007 CVS),
  the IR foundations (001/002/003), and the dependency-surface policy (009).
- **`encode` is gated** pending the prikk import surface and the owner's open questions (RFC 008); this
  build is decode + inspect + verify + summary.
