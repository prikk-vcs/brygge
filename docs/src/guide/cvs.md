# Importing a CVS repository

brygge imports a CVS repository's **main line** (the trunk, plus — while a vendor branch is set — the
revisions `cvs import` put there, since that is what a plain `cvs checkout` gives you). With
`--reconstruct-refs` it also imports the **branches cut from the main line**, each with its own history.

## What brygge carries

- The main line's full per-file content and history, reconstructed into changesets (every changeset atom
  is `Derived(ReconstructedChangeset)` — CVS has no atomic commit, so brygge's grouping is its own
  judgment, carried with its clustering parameters and a confidence).
- Main-line tags, with `--reconstruct-refs`.
- **Branches, with `--reconstruct-refs`** (see below): a ref of kind branch, and the branch's own changesets.

## Branches (with `--reconstruct-refs`)

A CVS branch is not one line of history: each file has its own branch, cut at its own revision, and nothing
says which commits belong together. A branch is *identified by its symbol name* (`cvs tag -b BR`), so branches
are imported only with `--reconstruct-refs`, and only the ones that have a name.

- **The branch tree.** The branch's changesets are clustered from that branch's revisions alone (the same
  rules as the main line), and each is a `Derived(ReconstructedChangeset)`. A branch's ref points at its last
  changeset; a branch with no commits is a ref at its parent.
- **The parent is the earliest covering changeset.** The changeset on the main line after which the largest
  number of the branch's files were at their branch-point revision, the earliest such if several tie. When
  every file of the branch was at its branch point right after one changeset, the parent is *exact*. When the
  files were tagged at different moments (for example, one by one with commits between), no single changeset
  is right: the parent is **approximate**, and brygge says so — the artifact carries a `ConventionViolation`
  flag ("CVS branch point spans reconstructed changesets") and the run exits `30`. This is a judgment, not a
  fact; the branch's **content is still exact** (each file's content on the branch is what
  `cvs checkout -r BR` gives).
- **The branch-point atom.** A branch may hold only some of the files (for example, a branch tagged from a
  subdirectory). Then the tree the parent had is not the tree the branch started from, and brygge adds an atom
  named `branch-point:<name>` between the parent and the first branch changeset. It is
  `Derived(ReconstructedBranch)` and only adds and deletes the paths that differ, so that the branch's tree
  equals what CVS gives. A branch that covers the whole tree has no such atom. A file added on the branch is
  added by its first branch revision; a trunk file added after the cut is not in the branch.
- **No merges are inferred.** CVS records none (a merge is a plain commit on the target), and brygge does not
  guess them from the content: a branch's atoms have one parent.
- **Tags on a branch revision** resolve to the branch changeset that contains that revision.

## What it does not carry

- Without `--reconstruct-refs`, branches are not imported at all: every revision on a branch other than the
  currently-set vendor branch is excluded and recorded as a drop (`CVS branch revisions not imported (…)`),
  never silently.
- **Unnamed branches**: revisions on a branch whose symbol was deleted have no name to identify the branch
  across files. They are dropped and recorded (`CVS branch revisions on unnamed branches not imported (…)`).
- **Branches cut from a branch revision** (a branch of a branch, or a branch cut from a vendor revision after the
  vendor branch was cleared): not imported yet. A symbol is imported only when it is cut from the **main line
  in every file that has it**; if it is cut from a branch revision in even one file (a mixed working copy), the
  whole symbol is not imported, and all its revisions are counted in one record (`CVS branches cut from a
  branch revision (N branches, M revisions)`). A branch that leaves such a branch's *parent* is still imported.
- **Vendor branches**: while the vendor branch is the file's default branch, its revisions are the main line,
  as for `cvs checkout`, and a branch cut from one of them (`cvs import`, then `cvs tag -b`) is a main-line
  branch like any other. After a file's vendor branch is cleared, later vendor-import revisions are recorded
  (`CVS vendor-branch revisions after the vendor branch was cleared (… revisions)`). A vendor branch's own
  symbol (e.g. `VENDOR:1.1.1`) is a literal odd-length number, not a "magic" one, and is not reconstructed as
  a branch.
- **A branch symbol naming a revision the file does not have** (for example after `cvs admin -o`): the file
  is not on the branch; recorded (`CVS branch symbols naming a missing revision (…)`).
- A **tag** that ends up naming no imported revision is recorded as its own drop
  (`CVS tags on branch revisions not reconstructed (…)`), never silently skipped.

## Repositories brygge refuses

A refusal exits `20` and names the file or symbol; nothing is written. Besides a remote (`:pserver:`)
source, a symlink, a path in both `Attic/` and live, and a non-UTF-8 path, two more shapes are refused
because there is no honest way to import them:

- **`default-branch-with-later-trunk`** — a file whose default (vendor) `branch` is still set, but which
  also has trunk revisions after that branch's branch point (possible with `cvs admin -b`). A checkout
  gives the branch tip while the trunk moved on, so the file has no single main line. In a **copy** of the
  repository, reset the default branch with `cvs admin -b` (no argument) on that file, or repair it by
  hand, then import the copy. The consequence: the file's main line then follows the trunk, and the vendor
  branch's revisions are counted as branch revisions and not imported.
- **`non-utf8-symbol-name`** — a tag or branch symbol whose name is not valid UTF-8 (shown with `\xNN`
  for the invalid bytes). Ref names in the IR are text and are never converted lossily. In a **copy** of the
  repository, rename or remove the symbol (`cvs rtag -d` / `cvs rtag -r`), then import the copy.

## Seeing a repository's branches before migrating

- `rlog -h file,v` **lists** a file's symbolic names, but does not mark which are branches — read the
  revision number yourself:
  - CVS's "magic branch number" form has `0` as the second-to-last component (e.g. `1.2.0.2` names the
    branch a plain revision would call `1.2.2`);
  - a **vendor** branch is stored literally instead, as an odd-length number of three or more components
    (`1.1.1`) — the admin section's `branch` field, while set, names it the same way.
- `grep -A20 '^symbols' file,v` shows the raw symbols table directly, for either form.

## If you need what is not imported

Keep the source repository — brygge never modifies or removes anything it reads — and use another tool for
the parts above that are recorded as drops.
