# Handoff — RFC 021 batch K: contract 0.4.0, an entry's content is a blob or a submodule (request 089)

**Governing:** RFC 021 D-1, D-2 and D-6, accepted 2026-11-26 (as revised by review 184). **On a topic branch,**
`batch-k-contract-0.4.0`, because it changes the IR contract, the reader and the writer for every decoder. Push the
branch only after approval, and name it.

**This batch adds no submodule source.** No decoder produces `Content::Submodule` until batch G. It makes the
contract, the reader, `verify`, `Edit`, `check` and every existing consumer ready for one, and **proves that nothing
else changes.**

## 0. Evidence plan first (N-3), with `uptime` for any timing

- **Level 1:**
  - every wire rule of §2, each malformed case refused by name;
  - a round trip of hand-built IRs holding submodules;
  - the replay's kind rule (§3);
  - the reader of 0.2.0 and 0.3.0 unchanged;
  - a new golden-bytes test for 0.4.0.
- **Level 2:** every existing test, with the old artifacts in `crates/brygge/tests/data/` still read.
- **Level 3: the same history.** Every corpus entry, in every mode its kind supports, decoded by
  `fixtures/previous-release/0.9.0/brygge` (contract 0.3.0) and by this branch (0.4.0). **The two `Ir` values are
  equal** (§5).
- **The deep mutation sweep:** the reader is an untrusted parser, and it changes.

## 1. The model (`brygge-ir` `model.rs`)

- **The type:** `pub enum Content { Blob { blob: BlobId, mode: u32 }, Submodule { commit: Vec<u8> } }`.
  - **Its doc comment defines the concept once:** a submodule is "a pinned reference to a commit in another Git
    repository; brygge never reads that repository".
- **`PathOp::Add`, `Modify` and `Replace`:** `{ path, content: Content, status }`. `Delete` is unchanged.
- **Constructors and accessors:**
  - keep the call sites readable, for example `Content::blob(id, mode)`;
  - `PathOp::content()` returns `Option<&Content>`, which is `None` for `Delete`;
  - **no accessor returns a `BlobId` for a submodule,** and none defaults a mode: a caller must match.
- **`IrBuilder::add_atom`:** checks a submodule's `commit` length (20 or 32 bytes, otherwise `Invariant`). The
  existing checks (a blob is known, the ops are sorted, a path is unique) apply to `Content::Blob` only.
- **The `ContentStore`:** unchanged. A submodule has no blob.

## 2. The wire (`artifact.rs`, `ir-artifact-format.md`)

- **`version::CURRENT` is 0.4.0.** The writer always writes 0.4.0 (D-2).
- **`PathOp` in 0.4.0:**
  - Add, Modify and Replace are `{1 path: text req, 2 content: Content req, 4 status req}`, with field 3 unused, and
    so refused if present;
  - Delete is unchanged;
  - `Content` is an enum record: variant 0 `Blob {1 blob: id32 req, 2 mode: uvarint req}`, variant 1
    `Submodule {1 commit: bytes req}`;
  - `commit`'s length is 20 or 32, otherwise `NonCanonical`.
- **The reader:**
  - **0.2.0 and 0.3.0:** the existing layout, mapped to `Content::Blob`;
  - **0.4.0:** the new layout only, with the old layout under 0.4.0 refused;
  - the version gate first, as today.
- **The digest and the canonical form:** the digest covers the logical content as in 0.3.0 (RFC 018), so a
  submodule's `commit` bytes are inside it. Any ordering rule that names `blob` is restated for `content`.
- **`ir-artifact-format.md`:** the `PathOp` table, the new `Content` table, the version list, and the worked example,
  re-done for 0.4.0, byte by byte, as the 0.3.0 example was.

## 3. `verify` (`commands.rs` `check_source_invariants`)

- **The replay's tree** becomes a map from a path to its kind (blob or submodule).
- **The rules:**
  - `Add` needs the path absent;
  - `Modify` needs it present, **with the same kind as its new content**;
  - `Replace` needs it present, with either kind;
  - `Delete` needs it present.
- **The new refusal's text:** "a Modify op changes a path's kind (a blob and a submodule); a change of kind is a
  Replace".
- **Level 1:** a Modify from a blob to a submodule is refused; a Replace is accepted.

## 4. Every consumer compiles, and none treats a submodule as a file

**The places to change** (list each in the request):
- `transform::Edit`: `set_op_blob` and `set_op_mode` act on `Content::Blob` and **refuse** a submodule by name;
- `transform::check`;
- `inspect` (`--atoms` prints `submodule <hex>`);
- `encode git`;
- the bench;
- the mutation tests;
- every decoder, which produces `Content::Blob`.

**`encode git` until batch G:** a `Content::Submodule` is a **pre-flight refusal**: "a submodule at <path>: this build
cannot yet write submodules". It is temporary, it is never silent, and batch G replaces it.

**The plugin guides** (`writing-an-encoder.md`, `writing-a-transform.md`):
- the two-line migration;
- the rule: "hold a submodule, or refuse it by name; never write it as a file".

## 5. The same history, proved (level 3)

- **A new `brygge-bench ir-equal <a> <b>`:**
  - it reads two artifacts of any supported contract and compares their `Ir` values. That covers the atoms, ops,
    refs, the loss record, the flags, the transforms and the provenance;
  - it reports the first difference, by atom source id;
  - the artifacts' contract versions are printed, not compared.
- **The runs:** every corpus entry, in every mode its kind supports, with 0.9.0's published binary and this branch's
  build. **Every pair is equal.**
  - Provenance differs only by `brygge_version` and `decoder_version`, which are equal before the version bump, as
    within any batch. Say so if it is not.
- **The old test artifacts** (`contract-0.2.0-*.ir`, `contract-0.3.0-cvs-repro-0.7.1.ir`) still read, and still give
  the same `--against-source` outcomes.

## 6. CHANGELOG and docs

- **`### Breaking`:**
  - "Artifacts are written as IR contract 0.4.0 (RFC 021): an entry's content is a blob or a submodule. brygge 0.9.0
    and earlier cannot read them; this build reads 0.2.0, 0.3.0 and 0.4.0";
  - "`brygge_ir::PathOp`'s `blob` and `mode` move into `content: Content`", with the migration.
- **`### Added`:** nothing user-visible until batch G.

## 7. Checks

- fmt, and clippy `-D warnings` on stable, `x86_64-pc-windows-gnu`, `x86_64-apple-darwin` and 1.87;
- `cargo test --workspace` summed, on stable and 1.87, plus the no-identity run;
- the deep mutation sweep;
- `tools/check-ir-isolation.sh`: `brygge-ir`'s closure is unchanged;
- links, the book, the release tools, `cargo deny`.

## 8. Request

- **File** `.git-exclude/review-requests/089-contract-0.4.0.md`, opening with the evidence plan.
- **Push the branch only after approval.**
