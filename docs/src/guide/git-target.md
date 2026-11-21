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

**An artifact with no refs at all** (a CVS decode without `--reconstruct-refs`, for one) still has its
commits written, but nothing points at them: they are unreachable, and lost the next time Git
garbage-collects. `encode git` warns about this on stderr; decode with the options that give the history
real refs first.

## The trailers

Every commit, and every annotated tag, ends with one block of trailers after a blank line, in this
fixed order (`git interpret-trailers --parse` reads it back):

| Trailer | When | Says |
|---|---|---|
| `Brygge-Encoded: brygge <version> from <git\|hg\|svn\|cvs>` | always | this history was written by brygge from another source, not recorded natively here |
| `Brygge-Source-Id: <id>` | commits only (a ref has no atom of its own) | the source's own id for this commit — printable ASCII as is, otherwise `hex:` and lowercase hex |
| `Brygge-Derived: <scope> <kind> <params>` | once per derived assertion the commit or one of its path operations carries | brygge's own judgment produced this, not the source's stated fact |
| `Brygge-Transformed: <scope> <step> <name> <version> (<changed>)` | once per layer of a `Transformed` status | a transform changed this, naming which one and what it changed |
| `Brygge-Supplied: <fields>` | when Git's format needed a value the IR did not carry | exactly what brygge invented to satisfy Git, never silently |
| `Brygge-Altered: <field> (<why>)` | when a value the IR *did* carry had to change to fit Git's syntax | what changed and why, never folded into `Brygge-Supplied:` |
| `Brygge-Artifact: sha256:<digest>` | always | the artifact this commit was built from, which holds the full loss record |

A `Transformed` mark on a ref with no message of its own to carry it (a lightweight tag, or a branch,
bookmark, named branch or `Other` ref) goes into the report instead, naming the ref.

## What this does not do (yet)

- **It does not check its own output** against the artifact (RFC 019 D-6, batch G2) — the one honesty
  claim this page cannot yet make for you. Run `brygge verify <artifact> --against-source <repo>` on the
  real source separately if you still have it.
- **It does not map identities** (RFC 019 D-4, batch G3): Subversion and CVS commits, which have no
  email, get `Brygge-Supplied: committer` (or `author`) and `<>` for the email, unless you run `brygge
  transform authors` first.
- Incremental or continuous synchronization, pushing to a hosting service, `git-svn` metadata, submodules
  and LFS, and SHA-256 object-format repositories are all out of scope for this release.
