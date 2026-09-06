# Handoff — `brygge-decode-hg` implementation (RFC 005)

**Realizes:** accepted **RFC 005** (Mercurial decoder), under **RFC 009** (dependency policy) and against
the **brygge-ir** core (RFCs 001/002/003). Read RFC 005's decisions D-1…D-8 first; this turns them into a
build. Follows the RFC 004 (`brygge-decode-git`) pattern closely — read that decoder and its handoff for
the shape; this document states only what differs for hg.
**Status.** Inherits Accepted; read tier ruled **Tier 2 (pure-Rust revlog reader)**, floor ratified
(refuse subrepos + largefiles + censored). Build against it now.
**Scope (increment 1 = ROADMAP M2):** `brygge decode hg` as a library — read a local Mercurial repository
by parsing its **revlog store directly** (no `hg` binary), and produce a `brygge_ir::Ir`, entirely
*Stated* **including source-recorded renames** (the point of M2). Plus the RFC 009 D-7 isolation test.
**Out (queued):** the CLI `decode hg` dispatch and `verify --against-source` for hg (small — the CLI
exists from RFC 004 increment 2 and dispatches on source kind); rename **inference** parity with Git
(OQ-A, ships **off**); large-repo streaming (OQ-E).

## 1. Crate & layout (RFC 009 D-1)

`crates/brygge-decode-hg/` — the only crate that reads hg. `#![forbid(unsafe_code)]`,
`#![warn(missing_docs)]`, workspace lints, 2018 module style, tests as siblings. Dependencies: a
**pure-Rust decompression** codec only — zlib via `flate2` (miniz_oxide backend, no C) and, if the store
uses it, a pure-Rust `zstd` decoder; **no `hg`, no C, no network crate.** Add them to
`workspace.dependencies` and the `deny.toml` allowlist; they are small, not a gix-scale review (RFC 005
D-1/OQ-A). `brygge-ir` and `verify --internal` link none of this.

Modules:
- `lib.rs` — crate docs, re-exports, `Error`, `decode(path, &Options) -> Result<Ir>`.
- `requires.rs` — read `.hg/requires`; support a **known set** of format requirements, **refuse unknown**
  ones with a named reason (the format-level safety gate — never misread an unknown variant).
- `revlog.rs` (+ tests) — the revlog reader: index + data, delta-chain reconstruction, decompression.
- `changelog.rs` — decode changelog entries → (manifest node, user, date, files, extra, description, p1/p2).
- `manifest.rs` — decode a manifest → `path → (filenode, flags)`.
- `filelog.rs` — decode a file revision; extract the **copy metadata** header (stated renames).
- `store.rs` / `open.rs` — locate `.hg/store`, `fncache`/`dotencode` path mangling, the floor checks.
- `decode.rs` — orchestrate → IR (mirrors the Git `decode.rs`).
- `options.rs` — `Options` (rename inference off by default; recorded into provenance).

## 2. The settled decisions (build to these)

- **Everything is `Stated`** except a rename brygge *infers* (opt-in, off by default — RFC 004 D-3).
  A source-recorded rename is `Stated` (D-3 below), **not** derived.
- **hg node id is never identity** — it goes into `SourceIdentity.atom_id` opaquely; the `AtomId` is
  brygge-ir's own SHA-256. Signatures likewise opaque, shown Unverifiable (SRC-H4/NG-3).
- **Refuse, never approximate**, below the floor and below the format line (§1 `requires.rs`).
- **No `hg`, no extensions, no hooks, no network, no config** (D-6): read the store bytes, nothing else.
- **Byte-deterministic** for the same repo + brygge version + options; independent of revlog physical
  packing (read logical revisions, not delta layout).

## 3. The mapping spec (hg → brygge-ir)

Build with `brygge_ir::builder::IrBuilder`. **Ordering:** changelog revisions are stored in insertion
order with parents at lower revision numbers, so **changelog-rev order is already parent-first** — iterate
it directly, threading a `hg-node → AtomId` map; assert each parent precedes its child (a violation is a
malformed store → typed error, not a panic). Per changeset:

- **Changeset → `ChangeAtom`** (`status = Stated`): `parents` = the `AtomId`s of p1/p2 (skip null parent);
  `metadata` = user/description/date as `MetadataClaims` (claims, PR-3); `source` =
  `SourceIdentity { kind: Hg, repo_id, atom_id: <changeset node>, signatures }` (`repo_id` = the smallest
  root changeset node, content-stable, as the Git decoder does).
- **Manifest diff (p1 manifest → this manifest) → `PathOp`s** (all `Stated`): added → `Add`, changed
  filenode/flags → `Modify`, removed → `Delete`, canonical path order; content = the file revision's
  reconstructed bytes → `builder.add_blob`. hg flags: `x` → exec mode, `l` → symlink, else regular —
  map to the IR mode as the Git decoder maps Git modes. Merge changeset: diff against **p1** (parity with
  the Git first-parent rule).
- **Stated renames (D-3, SRC-H2 — the point of M2):** when a file revision's data carries a **copy
  metadata header** (`\x01\n … copy: <from>\n copyrev: <node>\n … \x01\n`), emit a
  `RenameHint { from, to, status: Stated }` **beside** the literal ops — the source's own assertion,
  marked fact, never re-derived, never collapsed. A `copy` that keeps its source (no `Delete` of `from`)
  is a stated copy; a `rename` shows the `Delete(from)` too. Strip the metadata header before hashing the
  file's content blob (the stored bytes minus the header are the file content).
- **Refs → `RefRecord`** (`Stated`): **bookmarks** (`.hg/bookmarks`) → `RefKind::Bookmark`; **named
  branches** (the changeset `extra["branch"]`, default `default`) → `RefKind::NamedBranch`, one per branch
  head (multiple heads → multiple records); **tags** — carried as `.hgtags` **file content** only for M2
  (OQ-C); do not synthesize tag refs. Neither branch model is privileged (IR-6).
- **Provenance:** `decoder = "brygge-decode-hg"`, params = `Options`, `import_time = None` (as Git).

## 4. Floor & loss (D-4/D-5, owner-ratified)

- **Floor-refused, named reason, CL-08 outcome (FA-3):** **subrepos** (`.hgsub`/`.hgsubstate` present),
  **largefiles** (`largefiles` in `.hg/requires`, or `.hglf/` standins), **censored revisions** (the
  revlog per-revision censored flag). Also **unknown `.hg/requires`** entries (§1) — a format the reader
  does not implement is refused, not guessed.
- **Dropped-with-record (representation, PR-7):** revlog physical layout & delta chains, the dirstate &
  working copy, **phases** (`.hg/store/phaseroots`). **Advisory-unreliable (PR-8):** **obsolescence
  markers** (`.hg/store/obsstore`). Workflow ref namespaces as under RFC 004. Nothing silently omitted
  (PR-9); the annotated-loss discipline is the same as the Git decoder's.

## 5. The revlog reader (the one genuinely new piece)

Spec at design level; implement bounds-checked and panic-free (untrusted input, T-2/INV-2):

- A revlog is an index (`.i`) plus optional data (`.d`); with the **inline** flag the data lives in `.i`.
  Support **revlogv1** and generaldelta; read the compression from `.hg/requires`
  (zlib default; zstd if `revlog-compression-zstd`). Refuse a revlog version/flag the reader does not know.
- Each index entry gives: byte offset, compressed length, uncompressed length, **base rev**, link rev,
  **p1/p2 revs**, and the **node id**. A revision's content is either a full snapshot (base == self) or a
  **delta against its base**; reconstruct by walking the base chain and applying hunks, then decompress.
  Bound the chain length and the reconstructed size (a malformed store must not OOM or loop).
- Expose: iterate revisions in order; get a revision's (node, p1, p2, parsed content) by rev or node.
  Changelog, manifest, and each filelog are all revlogs read through this one reader.

## 6. Build order

1. `revlog.rs` on a fixture (built with the `hg` CLI): read the changelog, reconstruct a known revision,
   verify its node id hashes correctly. 2. `changelog.rs`/`manifest.rs`: decode metadata + tree. 3.
   `filelog.rs`: content + copy metadata. 4. `decode.rs`: assemble a linear history → `Ir`. 5. renames,
   refs, floor, loss. 6. determinism + isolation tests.

## 7. Tests & gates (acceptance checklist)

Fixtures built by driving the **`hg` CLI** in temp repos (test-time only; the decoder itself never runs
`hg`), pinned identity/dates, skip-if-`hg`-absent (as the Git tests skip without `git`). Cover:
- **Faithful core:** a repo (linear + a named branch + a merge + a bookmark) decodes; atoms/ancestry/ops/
  metadata match; all `Stated`; fidelity report shows **zero derived** with inference off.
- **Stated rename (the M2 property):** `hg mv a b` in a commit → a `RenameHint` with **`status =
  Stated`** (not derived), beside the literal delete+add; the fidelity report shows it under *stated*, and
  an equivalent Git import of the same shape would show it *derived* — the checkable difference (FL-10).
- **Branch models distinct:** a named branch → `NamedBranch`, a bookmark → `Bookmark`, neither collapsed.
- **Determinism (VF-1):** decode twice → identical `to_bytes`; after a store-rewriting `hg` operation that
  preserves logical history, still identical.
- **Floor (FA-3):** subrepo / largefiles / censored / unknown-`requires` fixtures each refuse with a
  named reason.
- **Loss:** phases and obsmarkers recorded (representation / advisory); nothing silently omitted.
- **Isolation (RFC 009 D-7):** the IR + `verify --internal` build/run with no `brygge-decode-hg` present.

**Gates (all, `--locked`):** `cargo fmt --check`; `cargo clippy --workspace --all-targets --all-features
--locked -D warnings`; `cargo test --workspace --locked`; `cargo deny check`; `cargo audit`.

## 8. Acceptance criteria

- `decode()` produces a byte-deterministic, mostly-*Stated* IR from an hg repo; **source-recorded renames
  are `Stated`** and inference is off by default (D-3).
- Named branches and bookmarks map to distinct IR ref kinds without privileging one (D-4).
- Subrepos, largefiles, censored revisions, and unknown format requirements are refused with named
  reasons; phases/obsmarkers are dropped-with-record; nothing silently omitted.
- hg node ids and signatures preserved opaquely; nothing reads as target-verified.
- The revlog reader is panic-free on malformed input. gix-free and `hg`-free: only pure-Rust
  decompression is linked; the isolation test passes.
- **D-8 check:** if the IR needed any new field/variant, it is recorded as a pre-freeze contract note;
  the expected result is **none** (the freeze-precondition evidence).
- All gates green.

## 9. Queued next (not this increment)

- CLI: extend `decode` to accept `hg` and `verify --against-source` to dispatch by the IR's source kind
  (external design CL-01/CL-04, small).
- Rename **inference** parity with Git (OQ-A); `.hgtags`→tag-ref synthesis (OQ-C); large-repo streaming
  (OQ-E).
- Then **RFC 006 (Subversion) → M3**. With Git and Mercurial both exercising the IR, the **RFC 003 D-7
  contract freeze** decision is due (D-8).
