# brygge-decode-cvs

brygge's **CVS source decoder** (RFC 007, milestone M4 — the last source on the difficulty gradient). Reads
a CVS repository's RCS `,v` files directly and reconstructs changesets, producing a [`brygge_ir::Ir`].

- **Tier R (RFC 007 D-1):** a pure-Rust RCS `,v` reader. **No `cvs` tool, no FFI, no network, and no
  decompression codec** (RCS is uncompressed text) — the crate links only `brygge-ir` and `sha2` (for the
  content-derived `repo_id`, RFC 007 corrections CR-08.1).
- **The defining property:** CVS has **no atomic commit**, so the changeset itself is reconstructed by
  clustering per-file revisions on (author, log message, time window). **Every `ChangeAtom` is therefore
  `Derived(ReconstructedChangeset)`**, carrying its clustering parameters and a confidence — the honest
  "lossy-but-labelled" verdict (SRC-C3). Per-file content and history are carried faithfully.
- **Main line only (owner ruling D-2):** the trunk, plus — while a vendor branch is set — that branch's
  own revisions; every other revision is excluded and recorded, never silently. See
  [`docs/src/guide/cvs.md`](../../docs/src/guide/cvs.md) for what that means for a migration and how to
  see a repository's branches beforehand; branch-aware threading is planned for 0.3.0.
- **Honest limits:** changeset-level `verify --against-source` is **not offered** — there is no CVS atom to
  round-check against; brygge offers per-file content correspondence and reconstruction determinism (VF-1),
  and says so before the run (VF-5). CVS records **no renames** (delete+add, no `CopyRecord`).
- **The floor (see the table below):** a whole-import-under-confidence-floor history, a `:pserver:`/remote
  source, a symlink, a path in both `Attic/` and live, a non-UTF-8 path or symbol name, and a file whose
  default branch makes its main line ambiguous are refused. A single under-confidence changeset is not
  refused: it is imported and flagged (the CLI exits 30).

The only crate that reads CVS (RFC 009 D-1); [`brygge_ir`] and `verify --internal` link none of it. It reads
untrusted input, so every parser is bounds-checked and panic-free. See `rfcs/handoffs/007-cvs-decoder/` for
the implementation handoff and the security review.

## Floor

What brygge refuses rather than approximates. Each refusal exits 20 and names its feature by a stable
identifier (lowercase, kebab-case — the same identifier means the same thing in every decoder); the
identifiers are also recorded in every artifact's provenance as `params["floor"]`.

| Identifier | What is refused | What you can do |
|---|---|---|
| `remote-source` | a `:pserver:` / `:ext:` CVSROOT or a URL; brygge never runs a `cvs` client or touches the network | copy the repository's `,v` files to a local directory (e.g. with `rsync`) and give brygge that |
| `whole-import-under-confidence-floor` | a history in which **no** reconstructed changeset reaches the confidence floor, so nothing could be imported as a changeset with any confidence | there is no option to import it anyway in this version; keep the source repository (bulk imports and commits with skewed clocks are the usual cause of low scores) |
| `path-in-attic-and-live` | the same repository-relative path present as both `dir/f,v` and `dir/Attic/f,v` — an inconsistent repository | in a **copy** of the repository, repair it (for instance with `cvs admin` or by hand) so each path is in one place, then decode the copy |
| `symlink-in-repository` | a symlink anywhere under the repository root, file or directory; brygge reads only the repository it is given and never follows links | replace the link with the real file or directory, or remove it, in a copy of the repository |
| `non-utf8-path` | a path component that is not valid UTF-8 (shown with each invalid byte as `\xNN`) | rename the file in a copy of the repository |
| `non-utf8-symbol-name` | a tag or branch name that is not valid UTF-8 (shown as `\xNN`) | rename the symbol in a copy of the repository (re-tag under a UTF-8 name and delete the old symbol) |
| `default-branch-with-later-trunk` | a file whose default (vendor) branch is set but which also has trunk revisions after that branch's branch point (possible with `cvs admin -b`), so its main line is ambiguous | in a **copy** of the repository, run `cvs admin -b` with no argument on the file to reset the default branch, then decode the copy. Consequence: the file's main line then follows the trunk, and the vendor branch's revisions are counted as branch revisions and not imported |

Not floor features: a file, file count or delta chain over a ceiling (below) is refused with exit 20, and a
malformed `,v` file is a read error (exit 1).

## Ceilings

Checked before the memory it protects is allocated; a hit is a typed refusal (exit 20), never an OOM or a
hang (RFC 010 D-4):

| Ceiling | Default | Checked |
|---|---|---|
| `,v` file size | 512 MiB | from filesystem metadata, before the file is read |
| `,v` files per repository | 5,000,000 files | while walking the repository tree, before a file over the count is read |
| RCS delta chain length | 1,000,000 revisions | while reconstructing a revision, guarding against a cyclic or unbounded chain |
