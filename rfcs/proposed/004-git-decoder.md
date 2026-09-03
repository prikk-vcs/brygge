# RFC 004 — Git decoder

**Status.** Proposed (2026-09-04). Drafted for owner review. Two decisions in this RFC are the owner's
and gate acceptance (`GOVERNANCE.md`): **adopting `gix` as brygge's first heavy dependency** (RFC 009
D-6 — owner approval + architect security review) and **the Git feature floor's contents** (OQ-3). The
engineering shape below — the crate boundary, the object→IR mapping, the derived-rename discipline, the
floor *mechanism*, determinism, and against-source verification — is settled and buildable the moment
those two rulings land. This RFC does not presume them.
**Tracks.** ROADMAP Phase A1 → milestone **M1 (0.1.0, Git decode → IR)**. Track A — not prikk-gated
(decode stands alone, PU-1/PU-6). Realizes prikk RFC 113's decoder side for Git.
**Touches.** A new `brygge-decode-git` crate (the first heavy-dependency crate, RFC 009 D-1); the `brygge
decode git` command (external design CL-01, FL-01); the against-source verify path (CL-04, FL-04, VF-2);
`deny.toml` (the `gix` tree enters the audited set); the threat model (`T-2/T-4/INV-2`, revisited for a
real decoder).
**Requirements.** SRC-G1/G2/G3; PR-2/4/5/7/9; HO-1/HO-2; NG-3/NG-5; IR-1/IR-2; VF-1/VF-2/VF-5; FA-1…FA-5;
OQ-3. External design FL-01/FL-04/FL-05/FL-06, CL-01/CL-04/CL-08, IX-02/IX-03/IX-04, CF-01/CF-03.

## Summary

Git is the first source and the gradient's easiest (requirements §7): a content-addressed, atomic-commit,
real-DAG history whose content, ancestry, and messages import faithfully **as claims**. The whole job is
to map Git's objects into the IR without adding precision Git did not have (NG-5) and without dropping
the one link back to the original (the commit SHA + signature, PR-4). Two things are genuinely hard and
are where this RFC spends its care: **identity inference** — Git records delete+create, never a rename,
so any rename brygge asserts is *its* judgment and must be marked derived (SRC-G2/HO-1) — and the
**feature floor** — the features brygge refuses rather than approximate (SRC-G2/FA-3), whose exact line
is the owner's (OQ-3). This RFC settles the honest, all-*Stated* core; makes rename inference an opt-in,
always-marked, parameter-recorded addition that never collapses the literal ops; ships the floor as a
*mechanism* with a *provisional* list for the owner to ratify; and pins the determinism and
against-source-check disciplines the milestone rests on.

## The constraints that scope this design

- **Add no precision Git did not have** (NG-5/IR-1): the faithful core is entirely *Stated*; every
  inferred rename is *Derived* and never replaces the delete+create the source recorded (HO-1).
- **Never present a Git signature as target-verified authorship** (SRC-G3/NG-3/FS-04): the GPG signature
  is preserved opaquely (PR-4) and verifies nothing in prikk.
- **Heavy dependency stays isolated and forbids unsafe** (RFC 009 D-1/D-3): `gix` lives only in
  `brygge-decode-git`; `brygge-ir` and the internal-verify path link none of it (INV-5/CT-05).
- **The source is untrusted input** (RFC 009 D-4/INV-2/T-2): read objects only; execute no
  source-provided code — no hooks, no filters/smudge, no submodule fetch, no network, no ambient config.
- **Byte-deterministic output** for the same brygge version + repo + params (VF-1), and determinism
  survives failure (FA-5). Physical packing must not change the result.
- **The floor is product scope, not engineering** (OQ-3/FA-3): brygge implements whatever line is set and
  refuses cleanly below it; it does not set the line.

## Decisions

- **D-1 — A new `brygge-decode-git` crate links `gix` (RFC 009 D-2 tier 1); nothing else in the workspace
  does.** `gix` is pure Rust (no C, `forbid(unsafe_code)` stays maximal), MIT/Apache-licensed (RFC 009
  D-5 clean), and reads Git's object database directly. Per RFC 009 D-1 the crate is the *only* place the
  `gix` tree is linked: `brygge-ir`, the encoders, and `verify --internal` build and run without it
  (tested, RFC 009 D-7). The crate exposes one narrow function — read a repository path, produce a
  `brygge_ir::Ir` (or a typed decode error) — and holds no honesty/verification logic of its own; all of
  that already lives in the source-independent core. **This is the owner-gated heavy-dependency adoption
  (RFC 009 D-6): its acceptance carries an architect security review of the `gix` tree and a `deny.toml`
  update, and is what moves this RFC to Accepted.**

- **D-2 — The object → IR mapping, entirely *Stated*.** The faithful core adds no judgment:
  - **Commit → `ChangeAtom`** with `status = Stated`. `parents` are the IR `AtomId`s of the commit's
    parents (order preserved, PR-2). The commit's own SHA, the repository identity (e.g. the root-commit
    id), and any signature go into `SourceIdentity` opaquely (PR-4); the atom's `AtomId` is the IR's own
    content hash (RFC 001 D-4 / 003 D-3), **never** the Git SHA.
  - **Tree diff (parent→commit) → `PathOp`s**, all `Stated`: an added path → `Add`, a changed
    blob/mode → `Modify`, a removed path → `Delete`. A **root commit** (no parents) yields `Add`s for its
    whole tree against the empty tree. Ops are emitted in canonical path order (RFC 003 D-5). A merge
    commit's ops are the diff against its **first parent** (Git's own convention), with the honest note
    that a merge's content resolution is not itself a source-stated per-file assertion beyond that.
  - **Blob → content store** by `BlobId` (SHA-256 of the *raw* bytes — no smudge/EOL filtering, D-6);
    ops reference blobs; identical blobs dedup.
  - **Author/committer/message/times → `MetadataClaims`** (claims, never verified, PR-3). Times are
    source-stated and identity-bearing.
  - **Signature → `SourceIdentity.signatures`**, opaque (PR-4/SRC-G3). Shown `Unverifiable`, verifying
    nothing in any target (FS-04/NG-3).
  - **Refs → `RefRecord`s.** Local branches → `Branch`; lightweight and annotated tags → `Tag` (an
    annotated tag's own object, message, and signature are preserved). All `Stated`. *Which* ref
    namespaces are carried vs dropped-with-record is **OQ-B**.

- **D-3 — Rename inference is opt-in, always marked *Derived*, and never collapses the literal ops.**
  Git records delete+create; brygge **always** preserves that literally (the `Delete` and the `Add`, both
  `Stated`). Rename detection is **off by default** for M1 — the honest, fully-*Stated* import needs no
  heuristic, and a guess brygge did not have to make is a guess it should not make silently (the project's
  own rule). When the operator enables it (a recorded parameter, CF-01), each detected rename is added as
  a **marked `RenameHint`** with `status = Derived(InferredRename)` carrying its parameters — the
  similarity threshold and the detection mode — so a reader tells brygge's judgment from Git's record
  **without re-running the heuristic** (HO-1). The hint sits *beside* the delete+create, never in place of
  it (RFC 001 D-3). The IR model already enforces this shape; this decision fixes the *policy* (default
  off, parameters mandatory when on). The detection algorithm and its default threshold are **OQ-A**.

- **D-4 — The feature floor: this RFC ships the *mechanism* and a *provisional* list; the owner sets the
  line (OQ-3).** On encountering a feature it will not approximate, the decoder **refuses with a named
  reason** and the "hit the floor" outcome class (CL-08/FA-3) — it never guesses. The decoder reads a
  **floor policy** (CF-03) rather than hardcoding product scope, so the line is set by policy, not by
  code. The **provisional floor proposed for owner ratification** (SRC-G2), each item *refused* unless
  the owner rules otherwise:
  - **submodules** (a pointer to another repository — out of this import's scope);
  - **octopus merges** beyond the target's parent limit (a prikk/OQ-2 ceiling; until set, carry all
    parents and let the encoder's floor decide, or refuse above N — owner's call);
  - **replace refs (`refs/replace/*`) and grafts** (they *rewrite* the DAG a reader would otherwise
    see — importing the rewritten view silently would be exactly the laundering the project forbids);
  - **shallow clones** (a truncated history that looks whole — an FA-1 partial masquerading as complete).

  For each, the alternative to *refuse* is *carry-with-a-derived/loss record*; which items move to that
  column is the owner's OQ-3 ruling. brygge implements whichever line is set.

- **D-5 — The Git loss boundary (HO-2/PR-7), every drop class-stated, nothing silent (PR-9).**
  Representation-class drops recorded in the `LossBoundary`: **packfile/delta layout and the physical
  object store** (read logically, D-6), the **index and working tree**, and **reflogs** (local, not
  history). Ref namespaces that are workflow/representation rather than authored history (remote-tracking
  refs, `FETCH_HEAD`, stash) are dropped-with-record pending **OQ-B**. Nothing in the never-silently-omit
  class is dropped without a record.

- **D-6 — Determinism and the untrusted-read guardrails are the same mechanism (VF-1 + INV-2, one
  control).** The decoder reads **logical objects, not physical packing**, so `gc`/`repack` does not
  change the output; the IR's canonical ordering and hashing (RFC 003 D-5/D-3) do the rest. It takes **no
  ambient input**: `gix` is configured with **no hooks, no filter/smudge/EOL or `.gitattributes`
  processing** (blob bytes are hashed raw), **no submodule fetch, no network, and no ambient
  credentials/global config** (RFC 009 D-4). This is one decision serving two ends: config-driven
  smudging would be both a determinism break (same repo, different `core.autocrlf` → different content)
  and an untrusted-input execution surface. `import_time`, if recorded, is provenance-only and outside
  the digest (RFC 003 D-4/ID-4), so re-runs are byte-identical. Re-running after a failure reproduces the
  result up to the failure point (FA-5).

- **D-7 — Against-source verification is defined by the preserved SHAs (VF-2), and it is the one verify
  mode that links a decoder.** Because every atom carries its Git commit SHA (D-2/PR-4), a third party
  runs `brygge verify --against-source <original-git> --import <ir>`: brygge re-reads the original objects
  (via `brygge-decode-git`) and confirms, atom by atom keyed on SHA, that the import **corresponds** —
  same content per path, same ancestry, same metadata claims — **without trusting brygge's earlier run**.
  This deliberately uses `gix`, and that does **not** violate RFC 009 D-1/INV-5: the property RFC 009
  protects is that a **target** checks a brygge import on its *own* surface — that is `verify --internal`
  (VF-3), which links no decoder. `--against-source` is brygge's own tool re-deriving from the source and
  is *expected* to link the source library. The report never conflates "corresponds to source" (VF-2)
  with "internally honest" (VF-3) (VF-4).

## Open questions

- **OQ-A — Rename-detection algorithm and default threshold** (feeds D-3, CF-01). Exact-content moves vs.
  similarity (and at what score), and whether copy detection is offered at all. *Leaning:* ship
  detection **off** for M1; when enabling lands, use content-similarity with a conservative default and a
  recorded parameter, and tune it only once the prikk (node-identity) encoder actually exercises inferred
  identity — the consumer that gives the threshold a fitness signal.
- **OQ-B — Which ref namespaces are carried, dropped-with-record, or floor-refused** (feeds D-2/D-5).
  Branches and tags are carried. Proposed: **notes (`refs/notes/*`)** carried as content or
  dropped-with-record; **remote-tracking refs, `FETCH_HEAD`, stash** dropped-with-record;
  **`refs/replace/*`** floor-refused (D-4). Owner/architect to confirm.
- **OQ-C — The floor's exact contents** (D-4) — the **OQ-3** ruling for Git specifically: which of
  submodules / octopus-beyond-N / replace+grafts / shallow are *refused* vs *carried-with-record*, and
  the octopus parent limit N (tied to prikk OQ-2). Product scope; the owner's.
- **OQ-D — Very large repositories** (ties to RFC 003 OQ-B): decode memory/throughput, and whether a
  streaming object read is needed for M1 or deferred. *Leaning:* defer; correctness and determinism
  first, performance once a real large repo demands it.

## Consequences

- Git decode → IR becomes buildable, delivering **M1 (0.1.0)** and giving the IR (RFC 001/003) its first
  real source — a precondition for the eventual contract freeze (RFC 003 D-7, which needs Git *and*
  Mercurial to exercise it).
- brygge's **first heavy dependency (`gix`) lands**, from the start inside the RFC 009 boundary (isolated
  crate, `deny.toml`, `cargo-deny`/`cargo-audit` gates) and reviewed on entry — the surface is watched
  before it grows.
- This RFC establishes the **decoder pattern** — crate layout, all-*Stated* object mapping, derived-only
  inference beside literal ops, the read-a-policy floor mechanism, and against-source verification — that
  RFCs 005–007 (Mercurial, SVN, CVS) follow, each answering the same questions in its own terms (IR-5).
- On acceptance, the immediate next artifact is the **`brygge-decode-git` program-design handoff** under
  `rfcs/handoffs/004-git-decoder/`, then the implementation toward M1.
