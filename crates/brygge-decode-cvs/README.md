# brygge-decode-cvs

brygge's **CVS source decoder** (RFC 007, milestone M4 — the last source on the difficulty gradient). Reads
a CVS repository's RCS `,v` files directly and reconstructs changesets, producing a [`brygge_ir::Ir`].

- **Tier R (RFC 007 D-1):** a pure-Rust RCS `,v` reader. **No `cvs` tool, no FFI, no network, no
  subprocess, and no decompression codec** (RCS is uncompressed text) — the crate links only `brygge-ir`.
- **The defining property:** CVS has **no atomic commit**, so the changeset itself is reconstructed by
  clustering per-file revisions on (author, log message, time window). **Every `ChangeAtom` is therefore
  `Derived(ReconstructedChangeset)`**, carrying its clustering parameters and a confidence — the honest
  "lossy-but-labelled" verdict (SRC-C3). Per-file content and history are carried faithfully.
- **Honest limits:** changeset-level `verify --against-source` is **not offered** — there is no CVS atom to
  round-check against; brygge offers per-file content correspondence and reconstruction determinism (VF-1),
  and says so before the run (VF-5). CVS records **no renames** (delete+add, no `RenameHint`).
- **The floor (RFC 007 OQ-B):** an under-confidence changeset is loudly flagged/refused; a `:pserver:`/remote
  source is refused (INV-3).

The only crate that reads CVS (RFC 009 D-1); [`brygge_ir`] and `verify --internal` link none of it. It reads
untrusted input, so every parser is bounds-checked and panic-free. See `rfcs/handoffs/007-cvs-decoder/` for
the implementation handoff and the security review.
