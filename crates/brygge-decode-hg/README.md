# brygge-decode-hg

brygge's **Mercurial source decoder** (RFC 005, milestone M2). Reads a local Mercurial repository's
revlog store **directly** — pure Rust, no `hg` binary, no subprocess (RFC 005 D-1, Tier 2) — and produces
a [`brygge_ir::Ir`].

- **Tier 2 (RFC 005 D-1):** a pure-Rust revlog reader (index + delta chains + zlib/zstd). **No linked
  Mercurial library, no FFI, no network, no subprocess.**
- **What it carries:** a **`Stated`** changelog spine; **`Stated`** renames (hg records a copy/rename
  directly in the filelog metadata, so an hg import shows fewer derived marks than Git's); bookmarks and
  named-branch heads as refs.
- **What it refuses (the floor, RFC 005 D-4):** `subrepo`, `largefiles`, `lfs`, and a censored revision
  (its content was deliberately removed upstream — refused rather than imported as a hole).
- **The format-safety gate (`requires`):** a repository whose `.hg/requires` names a format this build
  does not implement — `revlogv2`, `treemanifest`, `narrowhg`, or anything unrecognized — is **refused,
  never guessed**, before a single revlog byte is parsed.

The only crate that reads hg (RFC 009 D-1); `brygge_ir` and `verify --internal` link none of it. It parses
an untrusted store, so every parser is bounds-checked and panic-free. See
`rfcs/handoffs/005-mercurial-decoder/` for the implementation handoff and the security review.

## Ceilings

Checked before the memory it protects is allocated; a hit is a typed refusal (exit 20), never an OOM or a
hang (RFC 010 D-4):

| Ceiling | Default | Checked |
|---|---|---|
| Reconstructed revision size | 1 GiB | on every decompressed revlog chunk (zlib and zstd alike) and on the output of every `mpatch` delta application |

Every reconstructed revision's length is also checked against the length its index entry records — a
mismatch is a malformed store (`Error::Read`), not a ceiling.
