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
  The repository's identity (`repo_id`) is the smallest **root** node among the published changesets, so it
  equals the identity of an `hg clone` of the repository, and a secret root never decides it.
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
- **What it refuses (the floor — see the table below):** subrepositories, large-file storage, censored /
  ellipsis / externally stored / unknown-flag revisions, an unresolved merge in progress, and any path or
  changelog extra key that is not valid UTF-8 (shown with each invalid byte as `\xNN`, never lossily
  converted).
- **Every revision is verified against its node.** brygge preserves each changeset, manifest and file
  revision node as the source's identifier, so it re-hashes every revision it reads the way Mercurial does
  (SHA-1 over the two parent nodes in ascending order and then the revision's raw text, with a
  collision-detecting SHA-1) and refuses a store whose content does not hash to its node —
  `revision <node> does not match its content (corrupt or crafted store)`. Mercurial checks the same on
  every read. Every changeset whose node reaches the artifact (atoms, refs, and the repository identity, which
  is taken from a published root) is read, so every such node is verified.
- **The format-safety gate (`requires`):** a repository whose `.hg/requires` names a format this build
  does not implement — `revlogv2`, `treemanifest`, `narrowhg`, or anything unrecognized — is **refused,
  never guessed**, before a single revlog byte is parsed.

The only crate that reads hg (RFC 009 D-1); `brygge_ir` and `verify --internal` link none of it. It parses
an untrusted store, so every parser is bounds-checked and panic-free. See
`rfcs/handoffs/005-mercurial-decoder/` for the implementation handoff and the security review.

## Floor

What brygge refuses rather than approximates. Each refusal exits 20 and names its feature by a stable
identifier (lowercase, kebab-case — the same identifier means the same thing in every decoder); the
identifiers are also recorded in every artifact's provenance as `params["floor"]`.

| Identifier | What is refused | What you can do |
|---|---|---|
| `subrepo` | a repository with `.hgsub` / `.hgsubstate` (Mercurial subrepositories) | decode the sub-repositories separately; remove the subrepository entries in a copy if you only need the main history |
| `largefiles` | a repository using the `largefiles` extension | convert the large files to ordinary files in a copy of the repository, then decode again |
| `lfs` | a repository using the `lfs` extension | convert the large files to ordinary files in a copy of the repository, then decode again |
| `censored-revision` | a revision whose content was deliberately removed (censored) upstream | keep the source repository; there is no content to carry |
| `unfinished-merge` | a repository with an unresolved merge in progress (`.hg/merge/state2` present) | finish the merge (`hg resolve`, then `hg commit`) or abort it (`hg merge --abort`), then decode again |
| `non-utf8-extra-key` | a changeset whose extras contain a key that is not valid UTF-8 | none without rewriting the history; keep the source repository |
| `non-utf8-path` | a manifest path that is not valid UTF-8 (shown with each invalid byte as `\xNN`) | rename the file in a copy of the repository |
| `ellipsis-revision` | a revision flagged *ellipsis* (a narrow clone), whose stored node does not hash to its text and so cannot be verified | decode the full repository the narrow clone came from |
| `external-storage-revision` | a revision flagged as externally stored, which holds a pointer rather than the text its node hashes | convert the externally stored files to ordinary files in a copy of the repository |
| `unknown-revision-flag` | a revision carrying a flag bit Mercurial itself does not define | none; Mercurial refuses such a revision too |

Two related refusals are format errors rather than floor features, also exit 20: a `.hg/requires` entry this
build does not implement (`revlogv2`, `treemanifest`, `narrowhg`, or anything unrecognized), and a
`.hg/store/obsstore` in a format other than version 1.

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
