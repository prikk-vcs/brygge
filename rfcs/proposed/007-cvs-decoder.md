# RFC 007 — CVS decoder

**Status.** **PROPOSED 2026-09-09.** Not accepted; nothing may be implemented from it. Its owner-gated
questions — the **read tier** (OQ-A) and the **confidence floor** (OQ-B, the CVS analogue of every prior
floor and the one that decides *how lossy is too lossy to import*) — are unresolved, and per `GOVERNANCE.md`
the read tier is an owner decision (RFC 009). This RFC states the design and hands the owner the rulings.

**Tracks.** ROADMAP Phase A4 → milestone **M4 (CVS decode → IR)** — the **last source on the difficulty
gradient** (requirements §7: Git → hg → SVN → **CVS**, "no atomic commit at all"). Track A — not
prikk-gated (decode stands alone, PU-1/PU-6). Realizes prikk RFC 113's decoder side for CVS. Second
**post-freeze** source (after SVN): CVS must fit **additive-only within IR major 1** (RFC 003 D-7; see D-9).
**Touches.** A new `brygge-decode-cvs` crate (isolating the read path, RFC 009 D-1); the `brygge decode cvs`
command; the threat model (`T-2`/`INV-2`, a new untrusted-input parser); and — importantly — the **published
per-source faithfulness statement** (VF-5), because CVS is the one source for which brygge must say *before
the run* that a changeset-level faithful import is not on offer (SRC-C3).
**Requirements.** SRC-C1/C2/C3; PR-2/3/4/5; HO-1/HO-2/HO-4; NG-1/NG-3/NG-5; IR-2/IR-5/IR-6; VF-1/VF-3/VF-5
(**not** changeset-level VF-2 — see D-7); FA-1…FA-5; OQ-3; BN-5.

## Summary

CVS is the end of the gradient and the IR's deepest epistemic stress test. Where Git, Mercurial, and
Subversion all give brygge an **atomic source atom** — a commit or a revision the source itself recorded, so
the `ChangeAtom` is `Stated` and only *renames* (Git) or *branches* (SVN) are `Derived` — **CVS has no
atomic commit at all** (SRC-C1). A CVS "commit" that touched *N* files is *N* independent per-file RCS
revisions with no record tying them together. So for CVS **the changeset itself must be reconstructed**, by
clustering per-file revisions on author, log message, and a time window (the `cvs2svn` lineage, imperfect by
nature) — and **the `ChangeAtom` is therefore `Derived`, not `Stated`.** This is the atom-level use of the
epistemic status the IR was built for (IR-2: "a CVS reconstructed changeset and a Git commit may occupy the
same IR slot while being epistemically different things").

Two consequences define this RFC, and both are honesty positions, not engineering choices:

1. **The honest deliverable is lossy-but-labelled (SRC-C2/C3).** Content and per-file history import
   faithfully (RCS preserves them); the *changeset grouping* imports as **brygge's derived judgment** —
   reproducible (VF-1) and reviewable (VF-3), carrying its clustering parameters (PR-5) and a confidence,
   but **never claimed as CVS's own fact.** The fidelity summary must make this uncertainty **prominent, not
   buried** (SRC-C2): every atom reads `derived:reconstructed-changeset`.

2. **Changeset-level VF-2 is impossible, and brygge says so before the run (SRC-C3, D-7).** The
   round-checkability VF-2 offers for Git — re-derive a commit from the source and confirm correspondence —
   **cannot exist for a CVS changeset, because there is no source atom to check against.** brygge offers
   instead *per-file* content correspondence (the RCS revisions are real and checkable) plus reconstruction
   *determinism* (VF-1), and it publishes this limit as the CVS faithfulness statement (VF-5) so "faithful
   CVS import" is never a promise brygge made and broke.

The two genuinely hard pieces are **reading RCS safely** (the `,v` store — a new untrusted-input parser) and
**the changeset reconstruction itself** (the research problem, tuned by the confidence floor OQ-B).

## The constraints that scope this design

- **The changeset is `Derived`; per-file content and history are faithful (SRC-C1/C2/C3).** Every
  reconstructed changeset atom is `Derived(ReconstructedChangeset)` with its clustering parameters (PR-5) and
  a confidence; the per-file RCS revisions, their dates, authors, log messages, and content are carried
  faithfully as the ops and metadata within.
- **Uncertainty is prominent, never buried (SRC-C2, HO-4).** The recoverable fidelity report leads with the
  reconstruction: all atoms derived, the clustering window used, and how many changesets fell near the
  confidence floor. Honesty is in the object (every atom marked), not only in prose.
- **Never claim changeset-level VF-2 (SRC-C3).** The against-source surface for CVS re-runs the
  reconstruction (VF-1 determinism) and checks *per-file* content correspondence; it does **not** assert the
  changeset grouping corresponds to a CVS record, because none exists. This is stated on the surface and in
  the published faithfulness statement (VF-5), before the run.
- **Never alter content silently (NG-5).** RCS keyword expansion (`$Id$` etc.) and CVS `-kb`/text-mode
  translation are working-copy transforms; brygge carries the **stored (unexpanded) bytes** and records the
  drop.
- **Read without executing source-provided code, without network, deterministically (INV-2/INV-3, VF-1).**
  Read the `,v` files (including `Attic/`) directly; no `cvs` server protocol, no `CVSROOT` script
  execution, no network. Reconstruction is a pure function of the revisions and the recorded parameters.
- **The read surface stays isolated and license-clean; nothing brygge produces links it (RFC 009 D-1/D-5,
  BN-5).**
- **The IR is frozen at 1.0.0; CVS must fit additive-only within major 1 (IR-5/IR-6, RFC 003 D-7, D-9).**

## Decisions

- **D-1 — A new `brygge-decode-cvs` crate; nothing else reads CVS. Read tier owner-gated (OQ-A).** The
  RFC 009 D-1 isolation property generalizes. It exposes one narrow function — read a CVS repository (a
  directory of RCS `,v` files, with deleted files under `Attic/`), produce a `brygge_ir::Ir`.
  **Architect recommendation (OQ-A leaning): Tier R — a pure-Rust RCS `,v` reader.** The RCS file format is
  documented, text-based (`@`-quoted strings, an admin/delta/deltatext structure, ed-style RCS diffs for
  non-head revisions), and **uncompressed** — so, like SVN Tier D, the decoder needs **no decompression
  codec, no C, no FFI, no network, and no external tool** (it reads the repository's own files directly;
  there is no clean `cvs` "dump" equivalent, and the `cvs` client/server path is messier and needs a
  working copy). The likely outcome is a **third zero-new-dependency decoder.** The rejected alternatives
  (driving the `cvs`/`rlog` CLI; linking an RCS/CVS C library) are in OQ-A. The **two genuinely new pieces**
  are the RCS reader (§ the untrusted parser) and the changeset reconstructor (D-3).

- **D-2 — Read the per-file RCS revisions as the atomic input.** For each `,v` file, the reader yields, per
  revision: the RCS revision number (e.g. `1.3`, or a branch revision `1.1.2.1`), date, author, state
  (`Exp`/`dead` — `dead` marks a deletion), log message, and the reconstructed content (head is stored
  whole; earlier revisions via the RCS delta chain). Bounds-checked and panic-free on malformed input
  (untrusted, T-2/INV-2). `Attic/foo.c,v` is a file deleted on the mainline; its revisions still read.

- **D-3 — Changeset reconstruction: cluster per-file revisions into `Derived` atoms (SRC-C1, THE decision).**
  Group per-file revisions into changesets by **(author, log message, time proximity within a window)** — the
  `cvs2svn` lineage — order them by time, and thread parent/child by the per-file revision ancestry they
  share (PR-2). Each changeset →
  `ChangeAtom { status: Derived(Derivation { kind: DerivationKind::ReconstructedChangeset, by:
  "brygge-decode-cvs", params: {"window": "<seconds>", "cluster_keys": "author,log"}, confidence: Some(_) }),
  … }`. The **confidence** reflects cluster tightness (time spread, ambiguity, split/overlap); a changeset
  below the floor is refused (D-8/OQ-B). The atom's `SourceIdentity.atom_id` carries the **canonical set of
  per-file `(path@rev)` pairs** the changeset groups (opaque, PR-4) — the only identity a reconstructed
  changeset has. `metadata` = author/date/log as claims (PR-3); the changeset date is a chosen representative
  (e.g. the latest per-file date), recorded as such.

- **D-4 — Per-file operations within a changeset → `PathOp`s; content faithful (SRC-C3).** Within a
  reconstructed changeset, each participating file's revision maps to `Add` (first revision / re-add),
  `Modify` (a later revision), or `Delete` (a `dead`-state revision), in canonical path order, content →
  the store. These path ops are the *faithful* part: they carry what CVS actually recorded per file. File
  mode from RCS/CVS flags: `-kb` (binary) → regular; the executable bit if the RCS mode records it. **CVS
  has no rename** — a move is a delete+add with no relation, so brygge emits **no `RenameHint` by default**
  (worse than Git; a rename may only be *inferred* under the same opt-in, always-`Derived`, off-by-default
  discipline as Git — RFC 004 D-3 — if the owner wants it, OQ-C). Keyword/`-kb` handling per NG-5 (D-6).

- **D-5 — Tags and branches (per-file symbolic names) → `Derived` reconstructed refs.** A CVS tag is a
  symbolic name in each `,v`'s admin section mapping the tag to a per-file revision; a repo-wide tag is the
  *set* of those revisions. brygge reconstructs a tag as a `Derived` `RefRecord` (kind `Tag`) pointing at the
  changeset that best corresponds, recording that CVS tags are per-file and may straddle reconstructed
  changesets (an honesty caveat in params, like SVN's tag-immutability note). A CVS **branch** (a magic
  branch number, e.g. `1.1.0.2`) → a `Derived` branch `RefRecord`. Both reuse
  `DerivationKind::ReconstructedBranch` (the mechanism — reconstruction from per-file source data — matching
  the SVN D-9 decision); ref reconstruction is **opt-in, off by default** (as SVN's), the stated per-file
  spine always importing.

- **D-6 — The loss boundary, with reconstruction uncertainty prominent (SRC-C2, HO-2/HO-4, PR-9).** The
  fidelity report leads with the reconstruction: every atom `derived:reconstructed-changeset`, the clustering
  window, and a count of changesets near the floor. Dropped-with-record: RCS physical layout and delta
  encoding (Representation); keyword expansion / `-kb` text translation (Representation — stored bytes
  carried, NG-5); CVS `CVSROOT` administrative files (modules, notify, etc. — not history); locks and
  working-copy state. **Nothing in the never-silently-omit class is dropped without a record.** There is no
  `svn:mergeinfo` analogue; CVS records no merge tracking (a merge is just more per-file revisions).

- **D-7 — Determinism is the checkable property; changeset-level VF-2 is honestly absent (VF-1/VF-3, SRC-C3).**
  Reconstruction is a pure function of the per-file revisions and the recorded parameters (PR-5), so a
  re-decode is byte-identical (VF-1) — that reproducibility, plus `verify --internal` (VF-3), is the
  integrity brygge offers. **`verify --against-source` for CVS re-runs the reconstruction and checks
  *per-file content* correspondence (the RCS revisions are real), and explicitly does *not* claim the
  changeset grouping corresponds to a CVS record** — the surface says so, and the CVS faithfulness statement
  (VF-5) states it before the run. `import_time` stays provenance-only (RFC 003 D-4/ID-4).

- **D-8 — The floor: refuse reconstructions below a confidence bar (OQ-3/OQ-B, owner-gated).** SRC-C3's honest
  position needs a line: some CVS histories reconstruct cleanly, others are hopeless (pathological timestamp
  skew, unrecoverable ambiguity). The owner sets a **confidence floor**; a changeset (or an import) whose
  reconstruction confidence falls below it is **refused with a named reason** (FA-3, the CL-08
  convention/confidence outcome), never imported as if certain. brygge implements the line via the
  read-a-policy mechanism (CF-03); it does not set it. This is the CVS analogue of the SVN
  convention-violation floor, and it is the OQ-3 decision that most directly rules *who can migrate*.

- **D-9 — Fit IR 1.0.0 additive-only (RFC 003 D-7). Preliminary finding: fits, likely with zero change.**
  The IR anticipated CVS (the model names it): a `Derived` `ChangeAtom` status,
  `DerivationKind::ReconstructedChangeset` (doc: "Params must include the clustering keys"),
  `Derivation.confidence: Option<u8>`, `DerivationKind::ReconstructedBranch` for tags/branches, and
  `SourceKind::Cvs` all **already exist** in the frozen contract. The one candidate to confirm at the handoff:
  **per-file revision preservation (PR-4)** — a reconstructed changeset packs its `(path@rev)` set into the
  opaque `SourceIdentity.atom_id` (which the model doc already anticipates: "a CVS revision tag"); whether a
  *structured* per-op source-revision field is ever wanted is deferred until a consumer needs it (OQ-D
  discipline), and would be an **additive** field then, never a break. Anything that would require a 1.0.0
  reader to change is out of scope for M4.

## Open questions

- **OQ-A — The read tier (owner-gated; the central dependency decision).** Options, with costs:
  - **Tier R — pure-Rust RCS `,v` reader** — *architect recommendation* (D-1). No C, no FFI, no external
    tool, no network, and (RCS being uncompressed) no decompression codec — a likely **zero-new-dependency**
    decoder. Cost: a new untrusted-input parser (RCS format + ed-style delta application) to hand-roll,
    bounds-checked.
  - **Tier C — drive the `cvs`/`rlog` CLI as a subprocess.** Reuses CVS's own reader. Cost: CVS's
    client/server and working-copy model is awkward for a read-only bulk decode, a runtime tool dependency,
    and content extraction still wants the `,v` files or a checkout; a subprocess posture (re-triggers the
    security review, as SVN Tier D did).
  - **Tier L — link an RCS/CVS C library (FFI).** Cost: a heavy C dependency + FFI — the surface RFC 009
    most wants to avoid; **owner-only**, needs the RFC 009 D-6 security review. Not recommended.
- **OQ-B — The confidence floor (OQ-3, owner-gated) — the decision that most shapes the product.** What
  confidence (and what definition of it — time-spread threshold, ambiguity measure) is *too low to import*?
  Refuse the whole import, or refuse the individual under-floor changesets and import the rest with a loud
  record (the SVN OQ-B precedent — import-with-loud-derived-record)? *Leaning:* per-changeset, import the
  confident majority and refuse (or loudly flag) the under-floor ones with named reasons, so a mostly-clean
  CVS history is not blocked by a few ambiguous points — but this is squarely the owner's product call.
- **OQ-C — Rename inference for CVS.** CVS records no renames (worse than Git). Ship **off by default** with
  no inference (delete+add carried faithfully), matching the maximally-honest default; a Git-style opt-in,
  always-`Derived` similarity inference is a later increment only if wanted. *Leaning:* no inference in M4.
- **OQ-D — Per-file revision preservation (D-9).** Pack `(path@rev)` into the opaque `atom_id` for M4
  (leaning), or add a structured per-op source-revision field (an additive minor bump, deferred until a
  consumer needs it)?
- **OQ-E — Timestamp skew and clustering robustness.** CVS timestamps are client-side and can be skewed;
  the clustering window and tie-breaking must be deterministic and documented (PR-5). *Leaning:* a fixed
  default window with the value recorded in provenance; adaptive windowing deferred.
- **OQ-F — Large repositories / streaming** (ties to RFC 003 OQ-B and the prior sources). *Leaning:* defer;
  correctness and determinism first.

## Consequences

- CVS decode → IR becomes buildable, delivering **M4** and **completing the difficulty gradient** — the IR
  will have been exercised by all four named sources (Git, hg, SVN, CVS), the strongest possible evidence
  for PU-1/PU-3 and the freeze.
- The **atom-level `Derived` status** becomes a shipped, checkable property: every CVS atom reads
  `reconstructed-changeset` on the fidelity surface, and the "lossy-but-labelled" verdict (SRC-C3) is made
  concrete rather than promised — the hardest honesty ask in the requirements, delivered as a property.
- CVS follows the **RFC 004/005/006 decoder pattern** (isolated crate, faithful per-file spine, `Derived`
  layer beside it, read-a-policy floor) while introducing the new shape of a **`Derived` atom** and the
  **honest absence of changeset-level VF-2** — confirming the IR carries a source with *no atomic record at
  all* (IR-2/IR-5) without a contract change.
- On acceptance, the immediate artifacts are the **`brygge-decode-cvs` program-design handoff**; an
  **architect security review against `brygge-03`** for the new RCS untrusted-input parser (and any
  subprocess, if a tier other than R is chosen); and the **D-9 additive-fit confirmation** against the
  shipped `brygge-ir`; then implementation toward M4.
