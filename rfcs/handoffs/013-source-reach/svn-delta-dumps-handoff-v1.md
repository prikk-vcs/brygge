# Handoff — RFC 013 D-2: SVN delta dumps (svndiff version 0)

**Governing:** RFC 013 D-2 (accepted 2026-09-24), with **OQ-5** (MD5, see §4). **Crates:**
`brygge-decode-svn`, plus one additive accessor in `brygge-ir` (§5). **Batch S** of 0.3.0, independent of
batch H. One review request.

**Today** a node with `Text-delta: true` or `Prop-delta: true` is refused ("delta dump"). So is every
`svnrdump` dump, although the SVN guide's own remedy for `remote-source` is to run `svnrdump`. **Accept
delta dumps.** Decoding the fulltext dump, the `svnadmin dump --deltas` dump, and the `svnrdump dump` of one
repository must give **byte-identical artifacts**.

---

## 1. What the tools emit (checked by the architect, Subversion 1.14.5)

- `svnadmin dump` (fulltext) writes format **2**. `svnadmin dump --deltas` and `svnrdump dump` write format
  **3**. All are already accepted as formats.
- **Text deltas:** `Text-delta: true`. The body is svndiff: `SVN` + version byte `0`. For a new file, the
  delta is against empty. Subversion splits text into windows of at most 100 KiB.
- **Property deltas:** `svnrdump` marks **every** node with a property block `Prop-delta: true`, including
  adds (a delta against empty properties). `svnadmin --deltas` marks only changes. A property block may then
  hold `D <len>` / `<key>` entries, which delete a property.
- **The checksums:**

  | Header | `svnadmin` (fulltext and `--deltas`) | `svnrdump` |
  |---|---|---|
  | `Text-content-md5` | yes | yes |
  | `Text-content-sha1` | yes | **no** |
  | `Text-delta-base-md5` | when the base is non-empty | when the base is non-empty |
  | `Text-delta-base-sha1` | when the base is non-empty | **no** |
  | `Text-copy-source-md5` / `-sha1` | on a copy with text | not written |

  **`svnrdump` gives MD5 only.** That is why OQ-5 exists.

## 2. The node's base

- **A text delta and a property delta apply to the node's base:**
  - a node with `Node-copyfrom-*` (add or replace): **the copy source** at its revision;
  - a `change`: **the path's current entry** in the tree being built (earlier nodes of the same revision
    count, in dump order);
  - an `add` or `replace` without a copy source: **empty** text and **no** properties.
- **A delta whose base is missing** is `Error::Read` ("<path>: delta against a base not present in this
  dump; an incremental or partial delta dump cannot be decoded alone"). One cause is `svnadmin dump
  --deltas --incremental -r N:M`. It is never guessed.
- **The base text is SVN's text, not the IR blob.** For an `svn:special` file, the IR blob is the bare
  target, and SVN's text is `link ` + target (`props::symlink_target` strips exactly that prefix, so it
  is exactly reversible). Write **one** function giving an entry's SVN text, and use it for every base and
  every checksum.
- **The two mode properties are tracked separately.** Keep, per file, whether `svn:executable` and
  `svn:special` are set, not only the derived mode. A file can carry both. When a property delta deletes
  `svn:special`, the file must fall back to executable if `svn:executable` is still set. The derived IR
  mode stays as `props::file_mode` gives it today.

## 3. Parsing and applying

- **svndiff version 0.**
  - **The header:** `SVN` then the version byte.
    - A version of **1 or 2** is `UnsupportedFormat` by name ("svndiff version 1 (zlib)" or "… 2 (lz4)";
      OQ-4). The reason text says to re-dump with `svnadmin dump` (fulltext or `--deltas`), or with
      `svnrdump`, both of which write version 0.
    - Any other version, or a body shorter than the header, is `Read`.
  - **Each window:** the source view offset, the source view length, the target view length, the
    instructions length, and the new-data length (each a big-endian base-128 integer; the high bit means
    "more"); then the instructions; then the new data.
  - **Each instruction:** the top two bits are the opcode (`00` copy from the source view, `01` copy from the
    target, `10` new data, `11` invalid). The low six bits are the length; `0` means a length integer
    follows. Then, for the two copies, an offset integer.
    - **A copy from the target** reads from the target window built so far, **byte by byte**, and may
      overlap what it is writing (that is how runs are encoded).
  - **Every length and offset is checked before use,** with checked arithmetic:
    - an integer longer than fits `u64`, or than `usize` where it indexes;
    - the source view inside the base;
    - a source copy inside the source view;
    - a target copy's start before the current position;
    - the instructions exactly filling the target view length;
    - the instruction and new-data sections each consumed exactly;
    - opcode `11`;
    - truncation anywhere.

    Each is `Read`, naming the node path. There is **no panic** and **no allocation** before the target
    length has passed the ceilings (§3, *Ceilings*).
  - Put the svndiff reader in its own module (for example `svndiff.rs`): bytes and a base in, the text or a
    typed error out. It knows nothing of dumps.
- **Property blocks.** The parser returns a node's property block as either **full** (as today) or
  **delta**, keeping `K` entries and `D` entries in order.
  - A `D` entry in a non-delta block is malformed (`Read`).
  - **Applying a delta:** start from the base's properties. A `K` entry sets a property, and a `D` entry
    removes it.
  - **What must be resolved:** the two mode flags (§2), and the refusal of `svn:externals` (a `K
    svn:externals` is refused exactly as today). Loss classification runs on the entries that the block
    sets.
  - **Acceptance (§7):** the three dump forms give the same flags and loss records. Any legitimate difference
    is reported with its cause, not absorbed.
- **Ceilings** (`Limits`, one place, the RFC 010 D-4 pattern):
  - `max_node_text_bytes`, default **1 GiB** (as Git's `max_blob_bytes` and hg's `max_revision_bytes`),
    applies to **every** node's resulting text, fulltext or delta.
  - **Total:** the sum of all reconstructed delta targets is ceilinged by `max_dump_bytes`. A delta dump
    never expands past what brygge would accept as a fulltext dump, so a small delta dump cannot amplify
    into unbounded memory (T-8).
  - Each window's target length is checked against both ceilings **before** it is allocated.
  - Exceeding either ceiling is `ResourceLimit`.

## 4. Verification (C-2f for SVN) — OQ-5

- **Check every checksum header that is present**, on every node, **fulltext nodes included**:
  - `Text-delta-base-*` against the base's SVN text;
  - `Text-copy-source-*` against the copy source's SVN text;
  - `Text-content-*` against the resulting SVN text.
- A mismatch is `Read` ("<path>: <header> mismatch; the dump or its base is not what it states").
- **This is also the end-to-end check of our own delta application** on every real dump. That is why it
  covers every node, and why MD5 matters (it is `svnrdump`'s only checksum).
- **SHA-1:** `sha1-checked`, already a workspace dependency (a new edge for this crate only). A detected
  collision is `Read`.
- **MD5: RFC 013 OQ-5** (added 2026-09-24, pending the owner's ruling).
  - **Recommended:** `md-5` 0.10 (RustCrypto, the family of the `sha1` and `digest` crates already in the
    lockfile).
  - Confirm with `cargo tree -i md-5` that it adds **one** crate, and that `cargo deny check` passes (the
    license is MIT OR Apache-2.0).
  - Pin it in `[workspace.dependencies]` as the others are.
  - If the owner rules otherwise, the architect amends this section before you start it.
- MD5 and SHA-1 here are **consistency** checks, not authenticity. The dump is untrusted and can state
  any checksum it likes. Say so in the code comment and the guide. Do not call it a security control.

## 5. `brygge-ir`: read a blob back

- The base of a change is content already given to the builder. Add
  `IrBuilder::blob(&self, id: &BlobId) -> Option<&[u8]>`, a read accessor over the content store it
  already holds (`content.rs` has `get`).
- It is additive: **no IR-contract change**, no new dependency, and `tools/check-ir-isolation.sh`
  unchanged.
- Do **not** keep a second copy of file contents in the SVN decoder.
- CHANGELOG: an Added line for `brygge-ir`.

## 6. `RR-svn-special-toggle` closes

- **The rule:** when a node sets or clears `svn:special` and carries no text, recompute its content from the
  base's SVN text under the new flag:
  - **becoming special:** the content goes through `symlink_target`. Content without `link ` is `Read`,
    exactly as today;
  - **ceasing to be special:** the IR blob becomes the SVN text itself, `link ` + target.
- **Where it applies:** fulltext and delta dumps alike. It is a bug fix, and only repositories that hit it
  change.
- Do not edit `brygge-03`; the architect closes the residual at the cut (v0.6).

## 7. Tests

**Fixtures come from the real tools**, built by the tests' existing `svnadmin` helpers. `svnrdump` and
`svnmucc` come with `subversion`, so CI's Linux x86_64 job already has them.

1. **Three forms, one artifact.** Build a repository covering:
   - text edits;
   - a binary file and an empty file;
   - a file of more than 100 KiB, changed in its middle (multiple windows, source views that slide);
   - copies with and without edits, and a replace with a copy;
   - property add, change and delete on files and directories;
   - a symlink, retargeted;
   - an executable file;
   - a file with both `svn:special` and `svn:executable`, then one of them deleted.

   Decode its fulltext dump, its `svnadmin dump --deltas` dump and its `svnrdump dump file://…` dump. The
   three artifacts are `cmp`-identical.
2. **svndiff unit tests:**
   - each opcode;
   - an overlapping target copy (a run);
   - several windows;
   - an empty target;
   - every malformed case in §3 gives its typed error;
   - version bytes 1 and 2 give `UnsupportedFormat`, by name.
3. **A property test:** random bytes after a valid header, against random bases, give only typed errors
   (never a panic), and never allocate past the ceilings. Use a fixed seed and thousands of cases.
4. **Checksums:**
   - one byte flipped in a delta's new data (so the target MD5 mismatches);
   - a wrong `Text-delta-base-md5`;
   - a fulltext node with a wrong `Text-content-md5` (new behaviour);
   - a wrong `Text-content-sha1`.

   Each is `Read`, naming the header.
5. **The missing base:** `svnadmin dump --deltas --incremental -r 2:HEAD` gives the `Read` of §2.
6. **The special toggle:** `svnmucc propset svn:special '*'` on a file whose text is `link <t>`, and
   `svnmucc propdel svn:special` on a symlink. Content and mode match `svn cat` and `svn proplist` at each
   revision.
7. **Ceilings:** small `Limits` for the per-node and the total ceilings give `ResourceLimit`, before
   allocating.
8. **Regression:** every existing SVN fixture decodes byte-identically (two builds, compared with `cmp`).
9. **Bench:** the SVN bench corpus, before and after (time, since every node is now hashed; the peak). Plus
   the same corpus from a `--deltas` dump: the same IR, and its time and peak.

## 8. Docs and CHANGELOG

- **`docs/src/guide/svn.md`:**
  - delta dumps and `svnrdump` dumps are accepted, and the `remote-source` remedy (`svnrdump dump <URL> >
    repo.dump`, then decode the file) now works;
  - svndiff 1 and 2 are refused by name;
  - checksums are verified (consistency, not authenticity);
  - an incremental delta dump cannot be decoded alone;
  - remove the delta refusal and the `svn:special` toggle limitation.
- **Module docs** (`dumpstream.rs`, `source.rs`, the crate README) stop saying that deltas are refused.
  - brygge itself still never runs `svnrdump` and never touches the network (INV-3).
  - A local repository is still dumped in fulltext. Do not change that here.
- **CHANGELOG `[Unreleased]`:**
  - **Added:** SVN delta dumps (`svnadmin dump --deltas`, `svnrdump`); `IrBuilder::blob`.
  - **Changed:** dump checksums are verified; one SVN node's text is ceilinged at 1 GiB.
  - **Fixed:** setting or clearing `svn:special` without a text change.

## 9. Review request

`.git-exclude/review-request/029-svn-delta-dumps.md`. Include the parser's bounds list, mapped to the
tests; the `cargo tree` and `cargo deny` output for `md-5`; and the three-form `cmp` results. After
approval, commit and push, and append CI.
