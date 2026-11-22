# Migrating to Git

```
brygge encode git <artifact> --out <dir>
```

`brygge encode git` is a supported, built-in encoder (RFC 019): it writes a new, bare Git repository
from an IR artifact, with every commit labelled by trailers naming where it came from and what, if
anything, brygge supplied, altered, derived or carried across a transform. `git` is reserved — `encode
git` is never a plugin, and a `brygge-encode-git` on `PATH` is never run (`encode --list` shows it
shadowed by the built-in).

- **`<dir>` must not already exist.** brygge refuses otherwise, naming it.
- brygge verifies the artifact first, the same way every command that reads one does.
- The result is a **bare** repository: clone or push it, rather than checking it out in place —
  `git clone <dir> my-project`.
- **A sidecar report, `<dir>.brygge-report`,** is written beside the repository, in the machine format
  (`machine-output.md` §3.9): the artifact's digest, the brygge version, the default branch chosen, and
  every drop, flag, supplied and altered field the result carries. Keep it with the artifact and the
  repository.

## The mapping

| IR | Git |
|---|---|
| An atom | A commit |
| A `Branch`, `Bookmark` or Mercurial `NamedBranch` ref | `refs/heads/<name>` |
| A `Tag` ref, with an annotation | An annotated tag object, `refs/tags/<name>` |
| A `Tag` ref, without one | A lightweight tag, `refs/tags/<name>` |
| An `Other` ref | `refs/brygge/other/<name>` — outside both the branch and tag namespaces, so it never reads as either |
| A path's mode `100644`/`100755`/`120000` | A regular, executable, or symlink Git blob mode |
| Any other mode | Refused by name before anything is written — the IR has no submodule mode, so this should not occur |

**The default branch** (`HEAD`) is the first of `trunk`, `main`, `master` or `default` that the IR has as
a branch-like ref; otherwise the first branch-like ref in the artifact's own canonical order. With no
branch-like ref at all (an artifact of tags only, or none), `HEAD` names `refs/heads/main`, which does
not exist yet — the report says so.

**Refs that would collide** on one Git ref (two IR refs of different kinds sharing one name, for
example) are refused by name, naming both, before anything is written — brygge never picks one or
renames either. Likewise, every Git ref name must pass Git's own rules (`git check-ref-format`).

**Every atom must be reachable from at least one ref** — walked through parents from every ref's own
target. An artifact where some atom is not is **refused**, naming how many atoms and the first few by
source id, before anything is written: a result that would silently lose history once Git
garbage-collects is not a migration, and D-7's pre-flight exists to catch exactly this. The refusal's fix
hint depends on whether the artifact has any ref at all:

- **No refs at all**, for CVS or Subversion: decode with `--reconstruct-refs`, which gives the history
  its refs in the first place. For Git or Mercurial (which always carry the source's own refs): a
  transform removed the one that would have reached these atoms, named by step when known.
- **Refs exist, but some atoms still are not reached by any of them**: `--reconstruct-refs` was already
  given and already worked, so the refusal does not suggest it again; it says plainly that no
  reconstructed ref reaches these atoms.

For Git or Mercurial this split does not change the hint: a removed ref is the only way an atom there goes
unreached, whether or not other refs still exist.

**One known limit:** a Subversion repository with no `trunk`/`branches`/`tags` layout gets no refs at all,
even with `--reconstruct-refs` — there is no convention to reconstruct one from. Such a repository cannot
be encoded to Git in 0.8.0.

## The trailers

Every commit, and every annotated tag, ends with one block of trailers after a blank line, in this
fixed order (`git interpret-trailers --parse` reads it back):

| Trailer | When | Says |
|---|---|---|
| `Brygge-Encoded: brygge <version> from <git\|hg\|svn\|cvs>` | always | this history was written by brygge from another source, not recorded natively here |
| `Brygge-Source-Id: <id>` | commits only (a ref has no atom of its own) | the source's own id for this commit, in a form that source's own people would recognize |
| `Brygge-Derived: <scope> <kind> <params>` | once per derived assertion the commit or one of its path operations carries | brygge's own judgment produced this, not the source's stated fact |
| `Brygge-Transformed: <scope> <step> <name> <version> (<changed>)` | once per layer of a `Transformed` status | a transform changed this, naming which one and what it changed |
| `Brygge-Supplied: <fields>` | when Git's format needed a value the IR did not carry | exactly what brygge invented to satisfy Git, never silently |
| `Brygge-Altered: <field> (<why>)` | when a value the IR *did* carry had to change to fit Git's syntax | what changed and why, never folded into `Brygge-Supplied:` |
| `Brygge-Artifact: sha256:<digest>` | always | the artifact this commit was built from, which holds the full loss record |

A `Transformed` mark on a ref with no message of its own to carry it (a lightweight tag, or a branch,
bookmark, named branch or `Other` ref) goes into the report instead, naming the ref.

**`Brygge-Source-Id`'s form is specific to the source kind**, not a generic encoding of the raw id:

| Source | Form | Example |
|---|---|---|
| git, hg | 40 lowercase hex characters, no prefix — the object id or node itself | `Brygge-Source-Id: 4b825dc642cb6eb9a060e54bf8d69288fbee4904` |
| svn | `r<decimal>` — the revision number | `Brygge-Source-Id: r1234` |
| cvs | one `path@rev` pair per line: the first right after the key, each later one on its own continuation line (so `git interpret-trailers --parse` still reads the whole thing as one value); or, for a branch-point atom, the literal `branch-point:<name>` | `Brygge-Source-Id: a.c@1.1`<br>` b.c@1.2` |
| anything else | printable ASCII as is, otherwise `hex:` and lowercase hex | `Brygge-Source-Id: hex:01ff0041` |

An id that does not fit its kind's form (the wrong length for git/hg/svn; anything malformed for cvs —
not UTF-8, an empty pair, a pair with a leading or trailing space, or no `@` at all) falls back to the
generic `hex:` form instead, so one bad id never breaks the rest. An id named in a refusal message is cut
to 80 characters plus `…`, since a CVS changeset's id can otherwise run to a whole paragraph.

**The field vocabulary in `Brygge-Supplied:`/`Brygge-Altered:`** is `author`, `author-email`,
`author-time`, `author-time-zone`, `committer`, `committer-email`, `commit-time`,
`commit-time-zone`, `tagger`, `tagger-email`, `tag-time`, `tag-time-zone`. A bare field (`author`,
`committer`, `tagger`) means the **whole identity** was invented, or copied from the author because the
IR stated no distinct committer; `<field>-email` means only the email was missing — a weaker, narrower
claim. The two are never conflated: a reader must be able to tell "brygge invented this person" from
"brygge filled in an empty email" apart.

## How brygge checks the result

Before `encode git` writes anything, it decodes the repository it just wrote, in process, and compares
it with the artifact. Any difference refuses the write: nothing is left at `--out`, and nothing at its
report. The same comparison is also a standalone command, over a repository someone kept:

```
brygge verify <artifact> --against-target git <repository>
```

It is read-only (the repository's refs are listed the same way `encode git` itself runs `git`: no user
configuration, no hooks), and it may not be combined with `--against-source` — a run answers one
question, either "does this match the source" or "does this match the result", never both at once.

**What is compared, and how:**

- Each result commit is matched to one artifact atom by its `Brygge-Source-Id` trailer, read back with
  the same per-source-kind form the trailer was written in.
- **States, never ops.** The full path → content mapping after a commit must equal the full path →
  content mapping after its matched atom. Two decoders can represent the identical resulting tree with
  different operations — a Subversion replace against a Git modify, or a no-op modify Git never
  records — so operations are never compared, only the state they leave behind.
- Parents, author, committer and message are compared against exactly what this guide's own rules say
  the writer would produce: a field this encoder supplied or altered is checked against the documented
  replacement (`<>`, `+0000`, the author's own claim), never against the artifact's original, possibly
  absent, claim.
- Refs are compared by D-3's own mapping: names, kinds and targets, through the same atom matching; an
  annotated tag's tagger, time and message follow the same rules as a commit's; `HEAD` must name the
  default branch this guide's "The mapping" section describes.
- **Nothing extra.** A commit, ref or tag in the result beyond this mapping does not correspond — even
  one a later, unrelated branch added to a kept repository. A migrated repository that is still being
  worked on is no longer being compared against the artifact that produced it; check it before anyone
  builds on it.

A difference reports what changed, up to 20 of them, then a count; `verify --against-target` exits with
`verify`'s own failed-check code.

## Mapping authors

Subversion and CVS carry an author name but no email, and no distinct committer at all: without
mapping, `encode git` supplies a `<>` email (marked `Brygge-Supplied: author-email`) and copies the
author onto the committer (marked `Brygge-Supplied: committer, commit-time`, the whole identity). `brygge
transform authors` gives them real Git identities first (RFC 019 D-4):

```
brygge inspect --authors repo.ir > authors.map    # then fill in each right-hand side
brygge transform authors repo.ir --map authors.map --out mapped.ir --require-all
brygge encode git mapped.ir --out repo.git
```

- **`inspect --authors`** lists every distinct identity the artifact carries — as an atom's author or
  committer, or an annotated ref's tagger — already in the map file's own form, one line per identity,
  with its counts per role: redirect it to a file and fill in each right-hand side.
- **The map file:** one `<source> = <Name> <<email>>` mapping per line. `#` starts a comment, to the end
  of the line; blank lines are ignored.
  - **`<source>`** is a **bare name**, matching an identity with exactly that name and no email (the
    Subversion/CVS case), or `<Name> <<email>>`, matching exactly that name and email (Git/hg). Neither
    form ever matches the other — a bare name never matches an identity that has an email, and the
    reverse, and both sides compare by exact bytes.
  - **The target side** needs a non-empty name and a non-empty email, and may not contain a newline,
    `<`, `>` or NUL — the same rule `encode git` itself applies to a written identity, so a mapped
    identity is never altered later.
- **`transform authors <artifact> --map <file> --out <file> [--require-all]`** is the one built-in
  transform (`authors` is reserved like `encode`'s `git`: a `brygge-transform-authors` on `PATH` is never
  run, and `transform --list` shows it shadowed by the built-in). It changes only an atom's author and
  committer, and an annotated ref's tagger; every change it makes is marked `Transformed`, by RFC 018
  D-5's own rule. An identity the map does not cover is listed on stderr with its counts;
  `--require-all` refuses the whole transform (exit 2, nothing written) if any is left unmapped. A
  mapping line that matched nothing gives a warning naming the line — it is not an error, since one map
  may cover several repositories.
- **Every mapped commit says so:** `encode git`'s own trailers carry `Brygge-Transformed: atom <k>
  authors <version> (author)` (and `committer`, when that claim also changed) — nothing new in the
  encoder, since it is just rendering the mark the transform already wrote. Once a mapping supplies a
  real email, the `Brygge-Supplied: author-email` mark is gone, because the email now exists.

## What this does not do (yet)

- Incremental or continuous synchronization, pushing to a hosting service, `git-svn` metadata, submodules
  and LFS, and SHA-256 object-format repositories are all out of scope for this release.
