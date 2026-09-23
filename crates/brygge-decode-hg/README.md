# brygge-decode-hg

brygge's **Mercurial source decoder** (RFC 005, milestone M2). Reads a local Mercurial repository's
revlog store **directly** — pure Rust, no `hg` binary, no subprocess (RFC 005 D-1, Tier 2) — and produces
a [`brygge_ir::Ir`].

- **Tier 2 (RFC 005 D-1):** a pure-Rust revlog reader (index + delta chains + zlib/zstd). **No linked
  Mercurial library, no FFI, no network, no subprocess.**
- **The published view (owner ruling D-4, revised; corrections handoff §1):** brygge computes exactly
  what `hg clone` would transfer — every changeset with phase `>=` secret, and every changeset that is
  obsolete (named as a precursor in `.hg/store/obsstore`) and not an ancestor of anything non-obsolete or
  pinned (a bookmark target, a working-directory parent, or a local tag — `.hgtags` does **not** pin), is
  excluded. Only the published set becomes atoms; the fidelity report counts what it excluded. A
  repository with an unresolved merge in progress (`.hg/merge/state2`) is refused rather than guessed at.
- **What it carries:** a **`Stated`** changelog spine; **`Stated`** copies/renames, each with its **true**
  source atom resolved (RFC 011 D-6: p1, else p2, else the copy's own filelog linkrev, else a
  first-parent ancestry walk — never placed on a guess); bookmarks and named-branch heads as refs, both
  computed over the published set only. Every changelog extra except `branch` is carried as a labelled
  `Extra` — notably `close`, whose value `1` means the changeset closed its branch (`hg commit
  --close-branch`); a closed branch's head still says so in the object, even though it no longer heads a
  live branch ref. Copy-source steps 3–4 (the linkrev and the ancestry walk) serve stores written by
  Mercurial < 3.3, or by other writers: since 3.3 (issue4476) `hg commit` records a copy only when its
  source is in p1's or p2's manifest, so current Mercurial always resolves at step 1 or 2. Steps 3–4 are
  covered by unit tests over an in-memory graph, not a live store.
- **What it refuses (the floor, RFC 005 D-4):** `subrepo`, `largefiles`, `lfs`, a censored revision (its
  content was deliberately removed upstream — refused rather than imported as a hole), an unresolved
  merge in progress, and a changelog extra key or symbol/path that is not valid UTF-8 (shown as `\xNN`,
  never lossily converted).
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
| `.hg/store/obsstore` size | 64 MiB | before the whole file is read into memory |
| A small untrusted store file (`phaseroots`, `bookmarks`, `localtags`) | 64 MiB | before the whole file is read into memory; `.hg/dirstate` is read bounded to its first 40 bytes only, since only the working-directory parent nodes are ever needed |

Every reconstructed revision's length is also checked against the length its index entry records — a
mismatch is a malformed store (`Error::Read`), not a ceiling.
