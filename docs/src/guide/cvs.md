# Importing a CVS repository

brygge imports a CVS repository's **main line only**: the trunk, plus — while a
vendor branch is set — the revisions `cvs import` put there, since that is what a plain `cvs checkout`
actually gives you. Branch history proper is not imported yet.

## What brygge carries

- The main line's full per-file content and history, reconstructed into changesets (every changeset atom
  is `Derived(ReconstructedChangeset)` — CVS has no atomic commit, so brygge's grouping is its own
  judgment, carried with its clustering parameters and a confidence).
- Main-line tags, with `--reconstruct-refs`.

## What it does not carry

- Branch history: every revision on a branch other than the currently-set vendor branch is excluded and
  recorded as a drop (`CVS branch revisions not imported (…)`), never silently.
- Branch symbols (tags naming a branch, not a revision) are only ever considered with
  `--reconstruct-refs`; when it is on, a branch symbol is recorded as a drop
  (`CVS branch symbols not reconstructed (…)`), never reconstructed as a ref. This includes a **vendor**
  branch's own symbol (e.g. `VENDOR:1.1.1`) — RCS stores it as a literal, odd-length revision number
  rather than the usual "magic" form, but it is still a branch symbol, not a tag.
- A **tag** that ends up naming no main-line revision at all — a vendor release tag once the vendor
  branch has been cleared, or a tag on an ordinary (non-vendor) branch revision — is likewise not
  reconstructed. It is recorded as its own drop (`CVS tags on branch revisions not reconstructed (…)`),
  never silently skipped: main-line-only import makes this common, not an edge case.

Branch-aware threading is planned for 0.3.0.

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

## If you need branch history now

Keep the source repository — brygge never modifies or removes anything it reads — and wait for 0.3.0, or
use another tool for the branch-aware parts of the migration in the meantime.
