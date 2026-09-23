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
- **The floor (RFC 007 OQ-B):** an under-confidence changeset is loudly flagged; a whole-import-under-floor
  history, a `:pserver:`/remote source, a symlink, a path in both `Attic/` and live, and a non-UTF-8 path
  are refused (INV-3, CR-08.3/8.4/CR-03).

The only crate that reads CVS (RFC 009 D-1); [`brygge_ir`] and `verify --internal` link none of it. It reads
untrusted input, so every parser is bounds-checked and panic-free. See `rfcs/handoffs/007-cvs-decoder/` for
the implementation handoff and the security review.

## Ceilings

Checked before the memory it protects is allocated; a hit is a typed refusal (exit 20), never an OOM or a
hang (RFC 010 D-4):

| Ceiling | Default | Checked |
|---|---|---|
| `,v` file size | 512 MiB | from filesystem metadata, before the file is read |
| `,v` files per repository | 5,000,000 files | while walking the repository tree, before a file over the count is read |
| RCS delta chain length | 1,000,000 revisions | while reconstructing a revision, guarding against a cyclic or unbounded chain |
