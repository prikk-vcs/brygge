# Handoff — `brygge-decode-svn` implementation (RFC 006)

**Realizes:** accepted **RFC 006** (Subversion decoder), under **RFC 009** (dependency policy) and against
the **brygge-ir** core (RFCs 001/002/003), frozen at IR **1.0.0**. Read RFC 006's decisions D-1…D-9 first;
this turns them into a build. Follows the RFC 004/005 decoder pattern closely — read the `brygge-decode-hg`
decoder and its handoff for the shape; this document states only what differs for SVN.
**Status.** Inherits Accepted; read tier ruled **Tier D (pure-Rust dumpstream parser)**, floor ratified
(`svn:externals` refused; convention-violating layouts imported-with-loud-derived-record, not refused).
**D-9 confirmed SVN fits IR 1.0.0 with zero contract changes** — build against the frozen types as-is; add
no IR field or variant.
**Scope (increment 1 = ROADMAP M3):** `brygge decode svn` as a library — read a Subversion history from a
**dumpstream** (a user-supplied dumpfile, or a read-only local `svnadmin dump` brygge invokes) and produce
a `brygge_ir::Ir`: a `Stated` linear revision spine with `Stated` copies, plus an **opt-in** `Derived`
branch/tag layer. Plus the RFC 009 D-7 isolation test.
**Out (queued):** the CLI `decode svn` dispatch and `verify --against-source` (small — the CLI exists from
RFC 004 increment 2, dispatches on source kind); **delta-format dumps** (svndiff — refused with a named
reason this increment, see §4); large-repo streaming (OQ-F).

## 1. Crate & layout (RFC 009 D-1)

`crates/brygge-decode-svn/` — the only crate that reads SVN. `#![forbid(unsafe_code)]`,
`#![warn(missing_docs)]`, workspace lints, 2018 module style, tests as siblings. **Dependencies:
`brygge-ir` only.** The dumpstream is uncompressed framed plaintext, so — unlike hg — there is **no
decompression codec, no C, no new `workspace.dependencies`, no `deny.toml` addition.** The one external
touch is `std::process::Command` (safe std) to run `svnadmin` when brygge produces the dump; that is a
*producer* outside the trust boundary (BN-5), not a linked library. `brygge-ir` and `verify --internal`
link none of this.

Modules:
- `lib.rs` — crate docs, re-exports, `Error`, `decode(source: &Source, &Options) -> Result<Ir>`.
- `source.rs` — resolve the input: a **dumpfile path**, or a **local repository path** brygge dumps with a
  read-only `svnadmin dump` (fixed argv, no shell, no network). **Refuse a URL / remote source** with a
  named reason (INV-3): brygge never dumps over the network (`svnrdump` is out).
- `dumpstream.rs` (+ tests) — the dumpstream reader (§5a): format version, UUID, revision records, node
  records, property blocks. **Refuse an unknown format version and a delta dump** (§4).
- `tree.rs` (+ tests) — the running repository-tree model (§5b): apply each revision's node records to a
  `path → (kind, BlobId)` snapshot, so directory copies/deletes expand to file-level ops.
- `props.rs` (+ tests) — parse SVN property blocks; classify `svn:*` (§3, §4).
- `layout.rs` (+ tests) — the branch/tag convention policy (opt-in ref reconstruction, §3).
- `decode.rs` — orchestrate → IR (mirrors the hg/git `decode.rs`).
- `options.rs` — `Options`: `reconstruct_refs` **off by default**, the layout policy, recorded into
  provenance (PR-5).

## 2. The settled decisions (build to these)

- **The stated spine is always `Stated`; the branch/tag layer is `Derived` and opt-in.** The linear
  revisions and the literal directory operations (including a `svn cp` that *is* a branch/tag creation)
  are always `Stated`. Branch/tag *ref synthesis* is a separate layer the user turns on
  (`Options.reconstruct_refs`); when on, every reconstructed ref is `Derived(ReconstructedBranch)` (D-9).
- **The SVN revision number is never identity** — it goes into `SourceIdentity.atom_id` opaquely; the
  `AtomId` is brygge-ir's own SHA-256. The repository UUID → `SourceIdentity.repo_id`. **Encode the revnum
  as 8-byte big-endian** so the builder's `(source.atom_id, AtomId)` topo tiebreak orders numerically, not
  lexically (decimal ASCII would sort `"10" < "9"` — wrong).
- **Refuse, never approximate**, below the floor and below the format line (§4). A **convention-violating
  layout is imported-with-loud-derived-record**, not refused (OQ-B).
- **No network, no hooks, no source code executes** (D-7/INV-2/INV-3): read dumpstream bytes; if brygge
  produces the dump, `svnadmin dump` is a local read that fires no repository hooks. Treat the dumpstream
  as untrusted regardless of origin (§6).
- **Byte-deterministic** for the same dumpstream + brygge version + options; independent of the physical
  backend — **`svnadmin dump` emits the same dumpstream from FSFS or BDB, so the RFC 006 BDB concern does
  not arise under Tier D.**

## 3. The mapping spec (SVN → brygge-ir)

Build with `brygge_ir::builder::IrBuilder` (`new(provenance)`, `add_blob`, `add_atom(AtomDraft)`,
`add_ref`, `set_loss`, `finish`). **Ordering:** dumpstream revisions are strictly increasing, so iterate
in dump order; each atom's `parents = [prev revision's AtomId]` (r1 has no parent). Thread a `revnum →
AtomId` map. Per revision:

- **Revision → `ChangeAtom`** (`status = Stated`): `parents` as above; `metadata` from revprops —
  `svn:author` → author `Identity` (**may be absent** for anonymous commits: carry `None`, never
  fabricate), `svn:date` → `author_time`, `svn:log` → `message` (all claims, PR-3); `source =
  SourceIdentity { kind: Svn, repo_id: <uuid>, atom_id: <revnum, 8-byte BE>, signatures: [] }`.
- **Node records → `PathOp`s** (all `Stated`), against the running tree (§5b), canonical path order:
  - `add` (file) → `Add`; `change` → `Modify`; `delete` → `Delete`.
  - `add`/`replace` **with `Node-copyfrom-path`/`-rev`** → the file `Add`/`Modify` **plus** a
    `RenameHint { from: copyfrom-path, to: node-path, status: Stated }` beside it (SRC-S3 — the source
    recorded the copy). A copy whose source is `delete`d in the same revision is a move; a copy without is
    a copy — both carry the stated `RenameHint`; the `Delete(from)` (if present) is emitted as its own op.
  - **`replace`** (a node removed and a new one added at one path in one revision): the IR has no `Replace`
    op and **two same-path ops in one atom sort unstably** (the builder orders ops by path only). Represent
    the net path effect as a **single `Modify`** (+ the `RenameHint` if copyfrom) — the node-replacement
    distinction is a node-identity fact the IR deliberately does not carry (model D-4); if the replace
    changes kind (file↔dir), see the directory rules below.
  - File flags from props: `svn:executable` → exec mode bit; **`svn:special`** → an IR **symlink** (its
    content is `link <target>`; strip the `link ` prefix, store the target as the blob) — map to the IR
    mode as the git/hg decoders do.
- **Directory node records** are handled by the tree model (§5b), not emitted directly: a directory `add`
  with copyfrom expands to `Add`s (+ `RenameHint`s) for every file it brings in; a directory `delete`
  expands to `Delete`s for every live file beneath it; an **empty directory** has no file to carry →
  dropped-with-record (Representation), since the IR (like git/prikk) has no empty-dir entity.
- **Branch/tag layer** (only when `Options.reconstruct_refs` is on): from the tree model, a directory copy
  whose destination matches the layout policy's branch/tag roots yields a `RefRecord`:
  - branch: `{ kind: RefKind::Branch, status: Derived(Derivation { kind: DerivationKind::ReconstructedBranch,
    by: "brygge-decode-svn", decoder_version, params: {"layout": "<policy>"}, confidence: None }),
    target: <atom of the branch head revision> }`.
  - tag: `{ kind: RefKind::Tag, status: Derived(Derivation { kind: ReconstructedBranch, params:
    {"layout": "<policy>", "immutability": "not-guaranteed: an SVN tag is an ordinary directory and may
    carry post-creation commits"}, … }) }` — D-9's recommendation: reuse `ReconstructedBranch` (the
    derivation *mechanism*), let `RefKind::Tag` carry the tag-ness, and put the caveat in `params`.
  - a directory the policy **cannot resolve** is imported-with-loud-derived-record: no ref is fabricated,
    the unresolved copy is recorded on the loss/fidelity surface as a named convention violation (OQ-B),
    and the stated directory-copy ops still import.
- **Provenance:** `decoder = "brygge-decode-svn"`, `params` = the `Options` (incl. layout policy),
  `import_time = None`.

## 4. Floor & loss (D-4/D-5/OQ-B, owner-ratified)

- **Floor-refused, named reason, CL-08 outcome (FA-3):** **`svn:externals`** (reaches other repositories,
  INV-3); a **remote/URL source** (§1 `source.rs`); an **unknown dump-format version**; a **delta-format
  dump** (`Text-delta`/`Prop-delta` — svndiff is out of scope this increment: refuse with "re-dump without
  `--deltas`", since a brygge-produced `svnadmin dump` is fulltext by default). A **convention-violating
  layout is *not* refused** — it is imported-with-loud-derived-record (§3, OQ-B).
- **Dropped-with-record (representation, PR-7):** FSFS/BDB physical layout & the dumpstream framing;
  `svn:eol-style` and `svn:keywords` (the dump content is already the **stored normal form** — carry it
  verbatim, record that keyword/EOL expansion is dropped, NG-5); `svn:ignore`/`svn:global-ignores` and
  other working-copy hints; **custom (user-defined) properties** (revision- and node-level; OQ-D — no IR
  sidecar this increment); empty directories. **Advisory-unreliable (PR-8):** **`svn:mergeinfo`** —
  dropped-with-record, **never** promoted to a merge parent (SRC-S2). Nothing silently omitted (PR-9).

## 5. The two genuinely new pieces

Both bounds-checked and panic-free on malformed input (untrusted, T-2/INV-2 — §6).

**5a. The dumpstream reader (`dumpstream.rs`).** Parse `SVN-fs-dump-format-version: N` (support 1–3;
refuse otherwise) and `UUID:`; then a sequence of **Revision** and **Node** records, each a block of
`Header: value` lines followed by length-prefixed property and text content
(`Prop-content-length`/`Text-content-length`/`Content-length`). Parse the property block
(`K <len>\n<key>\nV <len>\n<value>\n … PROPS-END`). **Bound every declared length against the bytes
actually present** — never pre-allocate an attacker-declared size; validate `Content-length` equals
`Prop-content-length + Text-content-length`. Refuse a `Text-delta:/Prop-delta: true` record (§4).

**5b. The running repository-tree model (`tree.rs`) — the SVN-specific core.** SVN dumps a directory copy
*implicitly* (it does not re-list the copied subtree), so the decoder must maintain a live
`path → (kind, BlobId)` snapshot and evolve it revision by revision: apply `add`/`change`/`delete`; expand
a **directory copy** by cloning the copyfrom subtree *as it was at copyfrom-rev* (keep prior revisions'
trees, or a structure that can reconstruct them, to resolve `copyfrom-rev`); expand a **directory delete**
to the live files beneath it. This snapshot is what turns node records into the file-level `PathOp`s of §3,
and it is where branch/tag copies (§3 layer) are detected. Bound tree size and copy depth (a malformed
dump must not OOM or loop).

## 6. Security posture (tees up the RFC 009 D-6 / GOVERNANCE review against `brygge-03`)

Tier D adds no heavy dependency, but it adds a **new untrusted-input parser** and a **subprocess**, so the
review will check, and the build must satisfy:
- **Untrusted parser:** all of §5 bounds-checked, no `unwrap`/`expect`/indexing (workspace lints warn;
  keep clean), typed errors not panics; declared lengths and counts bounded against real input; tree/copy
  growth bounded.
- **Subprocess:** `svnadmin dump` invoked with a **fixed argument vector** (no shell, no interpolation of
  the repo path into a shell string), read-only, output streamed as untrusted bytes; a user-supplied
  dumpfile is treated identically. No network at any point; a remote source is refused (§4).
- **Isolation (RFC 009 D-7):** `brygge-ir` and `verify --internal` build and run with no `brygge-decode-svn`
  present.

## 7. Build order

1. `dumpstream.rs` on a fixture (built by driving the `svnadmin` CLI at test time): parse headers, one
   revision, one fulltext node. 2. `tree.rs`: apply adds/changes/deletes; then directory copy/delete
   expansion, checked against `svn ls`-derived expectations. 3. `props.rs`: classify properties. 4.
   `decode.rs`: assemble a linear history → `Ir` (stated spine only). 5. copies→`RenameHint`, symlinks,
   floor, loss. 6. `layout.rs` + the opt-in `Derived` branch/tag layer. 7. determinism + isolation tests.

## 8. Tests & gates (acceptance checklist)

Fixtures built by driving the **`svnadmin`/`svn` CLI** in temp repos (test-time only; the decoder itself
runs only `svnadmin dump`, and tests may instead feed a captured dumpfile), pinned identity/dates,
skip-if-`svn`-absent (as the git/hg tests skip without their tools). Cover:
- **Faithful core:** trunk-only linear history decodes; atoms/ancestry/ops/metadata match; all `Stated`;
  fidelity report shows **zero derived** with `reconstruct_refs` off.
- **Stated copy (SRC-S3):** `svn cp` / `svn mv` → a `RenameHint { status: Stated }` beside the literal ops.
- **Directory copy expansion:** `svn cp trunk branches/x` brings the whole subtree in as file `Add`s; a
  directory delete removes the subtree; empty dir → dropped-with-record.
- **Derived branch/tag (with `reconstruct_refs` on):** standard layout → `Derived(ReconstructedBranch)`
  branch and tag refs, tag carrying the immutability param; a **convention-violating** layout imports with
  the violation recorded loudly and **no fabricated ref** (OQ-B); fidelity shows `derived.reconstructed-branch=N`.
- **Determinism (VF-1):** decode the same dumpfile twice → identical `to_bytes`.
- **Floor (FA-3):** `svn:externals`, a URL source, an unknown format version, and a **delta dump** each
  refuse with a named reason.
- **Loss:** `svn:mergeinfo` recorded advisory-unreliable and never a parent; eol-style/keywords recorded,
  stored normal-form bytes carried; custom props recorded; nothing silently omitted.
- **Isolation (RFC 009 D-7):** the IR + `verify --internal` build/run with no `brygge-decode-svn` present.

**Gates (all, `--locked`):** `cargo fmt --check`; `cargo clippy --workspace --all-targets --all-features
--locked -D warnings`; `cargo test --workspace --locked`; `cargo deny check`; `cargo audit`.

## 9. Acceptance criteria

- `decode()` produces a byte-deterministic IR from an SVN dumpstream: a `Stated` linear spine with
  `Stated` copies; `reconstruct_refs` **off by default**, and when on, branch/tag refs are `Derived` with
  the convention in `params` and the tag-immutability caveat carried (D-4/D-9).
- Convention-violating layouts import-with-loud-derived-record (no fabricated ref); `svn:externals`, URL
  sources, unknown format versions, and delta dumps are refused with named reasons; `svn:mergeinfo` is
  advisory-dropped and never a parent; keyword/EOL expansion dropped with the stored bytes carried; nothing
  silently omitted.
- SVN revision numbers and UUID preserved opaquely (revnum 8-byte BE for stable ordering); nothing reads as
  target-verified.
- The dumpstream reader and tree model are panic-free on malformed input; no new crate dependency; the
  isolation test passes; the `brygge-03` security review (untrusted parser + subprocess) is satisfied.
- **D-9 holds in the build:** no IR field or variant was added (confirmed zero-change; the reconstructed
  tag reuses `ReconstructedBranch`). If the build somehow needs one, stop — it would be an *additive* minor
  contract event to raise with the owner, not a silent change.
- All gates green.

## 10. Queued next (not this increment)

- CLI: extend `decode` to accept `svn` and `verify --against-source` to dispatch by the IR's source kind
  (external design CL-01/CL-04, small).
- **Delta-format dumps** (svndiff): apply `Text-delta`/`Prop-delta` so `svnadmin dump --deltas` and
  captured `svnrdump` dumps are readable — the obvious follow-up once fulltext ships.
- Richer layout policies (custom/nested roots, per-project trunks — OQ-C); large-repo/dumpstream streaming
  (OQ-F); an IR sidecar for custom properties **only if** a consumer needs it (OQ-D — an additive minor
  contract event, owner-gated).
- Then **RFC 007 (CVS) → M4**, the last source on the gradient.
