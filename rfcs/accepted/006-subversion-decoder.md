# RFC 006 — Subversion decoder

**Status.** **Accepted (2026-09-08).** Both owner-gated decisions are ruled. The **read tier is Tier D —
a pure-Rust parser of the SVN *dumpstream*, fed by a user-supplied dumpfile or a read-only local
`svnadmin dump`** (chosen over hand-rolling FSFS/BDB, driving the `svn` CLI, or linking `libsvn` — the
smallest pure-Rust surface with no FFI and no network; OQ-A). The **SVN floor is ratified: a repository
that violates the trunk/branches/tags convention is imported with every branch/tag reconstruction
recorded as a `Derived` judgment the user must accept and the violation named loudly on the fidelity
surface — not refused** (widest honest migration reach; OQ-B), while **`svn:externals` is refused with a
named reason** (INV-3). Tier D adds no gix-scale heavy dependency, so acceptance carries no separate
*dependency* security review — but it introduces a **new untrusted-input parser** (the dumpstream) and a
**subprocess posture** (`svnadmin`), so the `brygge-decode-svn` handoff carries an **architect security
review against `brygge-03`** (GOVERNANCE security gate; RFC 009 D-6) plus the `deny.toml`/`cargo-audit`
gate for any decompression codec. Next artifact: the `brygge-decode-svn` program-design handoff, then
implementation toward M3.

**Tracks.** ROADMAP Phase A3 → milestone **M3 (Subversion decode → IR)**. Track A — not prikk-gated
(decode stands alone, PU-1/PU-6). Realizes prikk RFC 113's decoder side for Subversion.
**First source after the IR freeze (RFC 003 D-7, 1.0.0):** where RFC 005 (hg) was the freeze
*precondition*, RFC 006 is the freeze's first *test under discipline* — SVN must fit **additive-only
within IR major 1**, and anything that would break a 1.0.0 reader is deferred or refused, not done
(see D-9).
**Touches.** A new `brygge-decode-svn` crate (isolating the read path, RFC 009 D-1); the
`brygge decode svn` command (external design CL-01, FL-10); the against-source verify path (CL-04,
VF-2); `deny.toml` / a runtime-dependency (or external-tool) declaration, **depending on the tier
ruling**; the threat model (`T-2`/`INV-2`/`INV-3`, revisited for a dumpstream parser and for any
subprocess the tier introduces).
**Requirements.** SRC-S1/S2/S3; PR-2/3/4/5/8/9; HO-1/HO-2; NG-1/NG-3/NG-5; IR-2/IR-5/IR-6;
VF-1/VF-2/VF-5; FA-1…FA-5 (FA-2 especially — SVN is its archetype); OQ-3; BN-5. External design FL-10,
CL-01/CL-04/CL-08, IX-02/03/04, CF-01/CF-03.

## Summary

Subversion is the gradient's third source, and it is the one that **stresses the *derived* side of the
IR** the way Mercurial stressed the stated side. SVN is not "harder Git"; it is a different shape of
truth (SRC-SVN, "different, not easier"). Two facts define it:

1. **Revisions are atomic and global.** r1, r2, … are repository-wide, monotonic, and each touches a set
   of paths as one unit. So **ancestry and content import faithfully, as claims** — the same all-`Stated`
   discipline as RFC 004/005 — and SVN's linear revision spine is honest history brygge never has to
   guess (SRC-S1 first clause).

2. **Branches and tags are not first-class — they are directory copies by *convention*.** A "branch" is
   `svn cp /trunk /branches/x`; a "tag" is `svn cp /trunk /tags/y` (and, treacherously, SVN tags are
   *mutable* directories, not sealed points). **Branch identity is therefore never recorded and can only
   be reconstructed by convention — a `Derived` judgment, off by default, always marked, and refused or
   flagged where a repository violates the convention** (SRC-S1, HO-1, FA-2 — SVN is *the* archetype of
   "refused or flagged, never guessed").

SVN also *does* record one thing faithfully that helps: **path copies carry `copyfrom-path`/`copyfrom-rev`**,
so a move or a branch-creation is a **source-stated** directory operation brygge carries as `Stated`
(SRC-S3) — but a path copy is still **not node identity** (SRC-S3), so the same derived-marking
discipline as Git applies to any *rename* read off it. The two places this RFC spends its care are
therefore (a) **how to read SVN safely and deterministically without a heavy or unsafe surface** — the
tier question, harder here than for hg (§ D-1/OQ-A) — and (b) **the honest treatment of the things SVN
models weakly**: convention-based branches/tags, advisory `svn:mergeinfo`, properties that rewrite
working-copy bytes, and `svn:externals`.

## The constraints that scope this design

- **Carry the faithful truth first; reconstruct convention only as `Derived` (SRC-S1/S3, HO-1).** brygge
  always carries the linear revision sequence and the literal directory operations (including copies with
  their `copyfrom`) as `Stated`. Any interpretation of directory layout as branch/tag structure is a
  separate, `Derived`, opt-in layer, marked at the record with the convention assumed as its parameter
  (PR-5) — never promoted to `Stated`, never woven into the stated spine.
- **A convention-violating repository is refused or flagged, never silently mis-branched (SRC-S1, FA-2).**
  This is the SVN archetype of the floor: brygge does not pick an interpretation and hope. It refuses with
  a named reason, or imports with the violation loudly recorded as a derived judgment the user must accept.
- **Mergeinfo is advisory and frequently wrong; never promote it to ancestry (SRC-S2, PR-8).** It is
  dropped-with-record (`LossClass::AdvisoryUnreliable`) — the direct parallel to hg obsmarkers — and is
  never a merge parent.
- **Never alter content silently (NG-5).** SVN properties `svn:keywords` and `svn:eol-style` rewrite
  working-copy bytes ( `$Id$` expansion, EOL translation). brygge carries the **repository-stored
  (normal-form) bytes**, not the expanded working-copy form, and records that the expansion is dropped.
  Carrying expanded bytes would be a silent content change — the one thing decode must never do.
- **Read without executing source-provided code, without network, and deterministically (INV-2/INV-3,
  VF-1).** No `svn:externals` fetches (they reach other repositories — a network and trust surface, INV-3);
  no hook execution; no network for the read itself. Read logical revisions, not physical FSFS/BDB packing,
  so output does not depend on how the repository was stored.
- **The read surface stays isolated and license-clean, and nothing brygge produces links it (RFC 009
  D-1/D-5, BN-5).** `brygge-ir`, the encoders, and `verify --internal` build and run without the SVN crate;
  a prikk repository verifies a brygge import using only prikk's own surface (BN-5).
- **The IR is frozen at 1.0.0; SVN must fit additive-only within major 1 (IR-5/IR-6, RFC 003 D-7).** If SVN
  needs a construct 1.0.0 lacks, it is an **additive** change (a new optional field or a new enum variant an
  old reader ignores or refuses under the read gate) or it is **deferred** — never a breaking change to the
  frozen contract (D-9).

## Decisions

- **D-1 — A new `brygge-decode-svn` crate; nothing else in the workspace reads SVN. Read tier: Tier D,
  the dumpstream parser (owner-ruled 2026-09-08, OQ-A) — the central decision of this RFC.** Per RFC 009 D-1 the crate is the only place
  the SVN read path lives; the RFC 009 D-7 isolation property (already tested for Git and hg) generalizes.
  It exposes one narrow function — read a Subversion source, produce a `brygge_ir::Ir` (or a typed error).
  **The tier choice is genuinely harder than hg's, and the hg answer does not transfer.** For Mercurial,
  a pure-Rust on-disk reader (Tier 2) was the *clean* choice — revlog is a single, tractable format. For
  Subversion, a pure-Rust on-disk reader is the *reckless* choice: the native store is **FSFS** (a complex,
  multi-version format) with a legacy **BDB** backend that is effectively a Berkeley-DB C artifact — a large,
  versioned, untrusted parser, or an FFI dependency, exactly the surface RFC 009 exists to contain. The
  honest clean-equivalent for SVN is not the on-disk format but the **dumpstream**:

  > **Tier D — a pure-Rust parser of the SVN *dumpstream*, fed by a user-supplied dumpfile or a read-only
  > local `svnadmin dump` (owner-ruled 2026-09-08, OQ-A).** The dumpstream is a documented, stable,
  > framed format; parsing it is pure Rust, bounds-checked and panic-free on malformed input (untrusted —
  > a dumpfile may be attacker-controlled, T-2/INV-2). No FFI, no linked SVN library, **no network** (a
  > local `svnadmin dump` reads a local repository; `svnrdump` over a URL is **excluded** by INV-3). Any C
  > lives only in the `svnadmin` *producer* — a read-only subprocess outside brygge's trust boundary and
  > absent from the produced artifact (BN-5) — or is avoided entirely when the user supplies the dumpfile.
  > The parser must handle the dumpstream format versions (v1/v2/v3).

  The rejected alternatives and their costs are in OQ-A. Tier D adds **no gix-scale heavy dependency**, so
  it needs no separate *dependency* security review; but the dumpstream is a **new untrusted-input path**
  and `svnadmin` is a **subprocess**, so the handoff carries an architect security review against
  `brygge-03` (GOVERNANCE security gate; RFC 009 D-6), and any decompression codec passes the
  `deny.toml`/`cargo-audit` gate.

- **D-2 — Revision → `ChangeAtom`, `Stated`; the spine is linear.** Each SVN revision → one `ChangeAtom`
  with `status = Stated`. **`parents` is the immediately preceding revision** (SVN's repository history is a
  linear sequence; there is no native DAG — D-6). The revision number and the repository UUID go into
  `SourceIdentity` opaquely (PR-4/SRC-S) — the `AtomId` is the IR's own hash, never the SVN revnum.
  `svn:author` (which may be **empty** for anonymous commits — carried absent, never fabricated), `svn:date`,
  and `svn:log` → `MetadataClaims` (claims, never verified, PR-3). Revision properties beyond these → D-5.

- **D-3 — Changed paths within a revision → `PathOp`s, all `Stated`; copies are `Stated` via `copyfrom`.**
  A revision's path changes map in canonical path order: added → `Add`, content/prop change → `Modify`,
  deleted → `Delete`, replaced (delete+add at one path in one revision) → the literal delete+add. **A path
  with `copyfrom-path`/`copyfrom-rev` is a source-recorded copy** — carried as `Stated`, and, when it is a
  copy-then-delete-of-source (a move), accompanied by a `RenameHint { from, to, status: Stated }` (SRC-S3,
  IR-2). File flags: `svn:executable` → the IR executable mode; `svn:special` → an IR symlink — both
  `Stated` (D-5). File content → content store by `BlobId` (SHA-256 of the **repository-stored** bytes,
  NG-5); ops reference blobs; dedup.

- **D-4 — Branches and tags are `Derived` reconstruction over a stated spine; this is SVN's defining
  epistemic problem (SRC-S1, HO-1, FA-2).** brygge separates two layers cleanly:
  - **The stated layer (always):** the linear revisions and the literal directory operations, including the
    `svn cp` that *is* a branch/tag creation, all `Stated`. This layer never lies and never guesses; a
    reader who wants only the truth SVN recorded gets exactly it.
  - **The derived layer (opt-in, always-marked, off by default):** interpreting directory paths as branch
    and tag structure — `/branches/x` → a `Derived` `RefRecord`, `/tags/y` → a `Derived` tag — under a
    **configurable layout policy** whose assumed convention is recorded as the derivation's parameter (PR-5).
    Standard `trunk`/`branches`/`tags` is the default policy, but many real repositories violate it, so:
    - a repository whose layout the active policy **cannot resolve is imported with the violation recorded
      as a `Derived` judgment the user must accept and named loudly on the fidelity surface — not refused**
      (owner-ruled 2026-09-08, OQ-B: widest honest migration reach; the stated linear spine always imports,
      only the interpretation is marked and consented, FA-2/HO-1/FL-10). The refuse-outright posture was
      considered and rejected as narrowing migration further than honesty requires; the derived record,
      loudly surfaced, keeps the same "never silently mis-branched" guarantee (SRC-S1);
    - a **`Derived` tag carries the recorded caveat that SVN tags are not guaranteed immutable** (they are
      ordinary directories and may have been committed to after creation) — an honesty fact 1.0.0 must be
      able to express (D-9 candidate).
    brygge **never** promotes a derived branch/tag to `Stated`, and never fabricates a branch a strict
    reading of the copies does not support. A false branch reconstruction is precisely the manufactured
    verification the whole project forbids (INV-1) — the same rule as Git rename inference (RFC 004 D-3).

- **D-5 — SVN properties → mapped, dropped-with-record, or refused; every drop class-stated (PR-9).**
  - `svn:executable` → IR executable mode (`Stated`); `svn:special` → IR symlink (`Stated`).
  - `svn:mergeinfo` → **`AdvisoryUnreliable`, dropped-with-record, never a merge parent** (SRC-S2/PR-8).
  - `svn:externals` → **refused with a named reason** (owner-ruled 2026-09-08, OQ-B): it references
    other repositories/paths — a network and trust surface, the SVN analogue of submodules/subrepos
    (INV-3; parallels RFC 004/005 submodule/subrepo floors).
  - `svn:eol-style`, `svn:keywords` → **workflow / representation, dropped-with-record**: they rewrite
    working-copy bytes; brygge carries the stored normal-form bytes and records the expansion as dropped
    (NG-5, the silent-content-change guardrail).
  - `svn:ignore`, `svn:global-ignores`, and other working-copy hints → workflow, dropped-with-record.
  - **Custom (user-defined) properties** → dropped-with-record for M3 (no consumer, and the frozen IR
    should not grow a speculative sidecar — D-9/OQ-D); revisiting is OQ-D.

- **D-6 — The loss boundary and the no-DAG reality (HO-2/PR-7/PR-8, every drop class-stated PR-9).**
  Representation-class drops: FSFS/BDB physical layout, dumpstream framing, delta storage. AdvisoryUnreliable:
  `svn:mergeinfo`. Workflow: eol-style/keywords/ignore. **The defining drop: SVN has no DAG, and brygge does
  not synthesize one.** The IR carries the linear revision ancestry (`Stated`) plus the opt-in derived
  branch/tag layer (`Derived`); brygge **never** manufactures merge parents from mergeinfo (that would
  promote advisory data to ancestry — forbidden, SRC-S2/INV-1). Nothing in the never-silently-omit class is
  dropped without a record.

- **D-7 — Determinism and the untrusted-read guardrails are one mechanism (VF-1 + INV-2/INV-3, as RFC
  004 D-6 / RFC 005 D-6).** Read logical revisions, not physical packing, so output is independent of how the
  repository was stored or dumped. Take **no ambient input**: no SVN client config, no `svn:externals`
  fetch, no hooks, no network. The dumpstream parser is bounds-checked and panic-free on malformed input
  (untrusted source, T-2/INV-2). `import_time` stays provenance-only (RFC 003 D-4/ID-4). Re-running after a
  failure reproduces the result up to the failure point (FA-5).

- **D-8 — Against-source verification by the preserved (UUID, revision) identifiers (VF-2, as RFC 004/005
  D-7).** Every atom carries its repository UUID and revision number (D-2/PR-4), so
  `brygge verify --against-source <svn-source>` re-derives from the source with the artifact's recorded
  options (including the layout policy, PR-5) and confirms correspondence, without trusting the earlier run.
  This mode links `brygge-decode-svn`; `verify --internal` still links no decoder (RFC 009 D-1). Caveat: the
  "source" for this mode is whatever the tier reads — a dumpfile or a local repository — and must be the same
  one; the against-source guarantee is relative to that input (stated on the surface, VF-5).

- **D-9 — The freeze test: SVN must fit additive-only within IR major 1 (RFC 003 D-7). This RFC is the
  freeze's first exercise under discipline.** **CONFIRMED against the shipped `brygge-ir` (40181f3): SVN
  fits IR 1.0.0 with ZERO contract changes — not even an additive minor bump** (full finding:
  [`handoffs/006-subversion-decoder/d9-additive-fit-confirmation.md`](../handoffs/006-subversion-decoder/d9-additive-fit-confirmation.md)).
  Every construct maps onto an *existing* 1.0.0 type: a linear `Stated` spine, `Stated` copies via
  `RenameHint`, `LossClass::AdvisoryUnreliable` for mergeinfo, `SourceKind::Svn`, and — for the derived
  branch/tag layer — the purpose-built `EpistemicStatus::Derived(Derivation { kind:
  DerivationKind::ReconstructedBranch, params, … })`, whose taxonomy doc already names SVN and mandates the
  convention in `params`. The three candidate needs are all **already expressible**, not additions:
  1. **convention as a parameter (PR-5)** — carried per-ref in `Derivation.params`; the `ReconstructedBranch`
     variant is defined for exactly this. No change.
  2. **"tag not guaranteed immutable" caveat** — carried in the derived tag's `Derivation.params`,
     recoverable from the object (HO-4). No change. One within-contract decoder choice remains — which
     `DerivationKind` a reconstructed *tag* uses (recommend reusing `ReconstructedBranch`; a first-class
     `ReconstructedTag` is the deferred additive if a consumer ever needs the distinction, OQ-D discipline).
  3. **convention-derived vs first-class branch** — already carried by `status` orthogonally to `kind`
     (`RefKind::Branch` + `Derived` vs `+ Stated`); a new `RefKind` would wrongly fold status into kind. No
     change.

  The derived branch/tag layer surfaces on the fidelity report (FL-10): `honesty::summary` counts derived
  ref statuses by taxonomy label, so a reconstructed branch shows as `derived.reconstructed-branch=N`.
  **Nothing SVN needs would require a 1.0.0 reader to change to stay correct**, so nothing is deferred on
  additive grounds — the strongest freeze outcome. (A separate, non-blocking note in the finding flags a
  doc/message refinement to `version.rs`'s forward-compat story, unrelated to SVN.)

## Open questions

- **OQ-A — The read tier** — **RESOLVED 2026-09-08 (owner-ruled): Tier D, the dumpstream parser** (D-1).
  The options weighed, with costs:
  - **Tier D — dumpstream parser (pure Rust), fed by a user-supplied dumpfile or a read-only local
    `svnadmin dump`** — **chosen.** Smallest pure-Rust honest surface; no FFI, no
    linked SVN library, no network; any C confined to the `svnadmin` producer outside brygge (BN-5). Cost:
    depends on `svnadmin` being available at decode time (a tool, not a linked dependency, and not in the
    artifact), or on the user producing a dumpfile; the dumpstream has format versions (v1/v2/v3) the parser
    must handle.
  - **Tier F — pure-Rust FSFS reader (no external tool).** No decode-time tool dependency. Cost: a large,
    multi-version, untrusted on-disk parser to hand-roll (and no answer for legacy **BDB** repositories
    without a Berkeley-DB dependency) — the biggest and riskiest surface of the three, against the
    clean/safe/small-surface line.
  - **Tier S — drive the `svn`/`svnadmin` CLI read-only as a subprocess** (e.g. `svnadmin dump` then parse,
    or `svn log`/`cat`). Faster to build. Cost: a runtime tool dependency and a subprocess trust/portability
    surface; brygge must invoke with no config, no hooks, no network (INV-2/INV-3).
  - **Tier L — link `libsvn` (FFI).** Complete and battle-tested. Cost: a heavy C dependency and FFI — the
    surface RFC 009 most wants to avoid; **owner-only**, and would require the RFC 009 D-6 security review.
    Rejected. RFC 009 and GOVERNANCE make this an owner decision because the decoder's dependency surface is
    the project's defining risk; the owner chose the smallest pure-Rust surface (Tier D).

- **OQ-B — The SVN floor contents (OQ-3)** — **RESOLVED 2026-09-08 (owner-ruled):**
  - **`svn:externals` → refused** with a named reason (reaches other repositories, INV-3).
  - **A layout the active convention policy cannot resolve → imported with the violation recorded as a
    `Derived` judgment the user must accept and named loudly (D-4), not refused** — the widest honest
    migration reach; the stated linear spine always imports, only the interpretation is marked and consented
    (SRC-S1/FA-2/HO-1). Refuse-outright was considered and rejected as narrowing migration further than
    honesty requires.
  - Still deferred to the handoff, not floor-blocking: BDB-backend repositories the dump path cannot read,
    and very old/partial dump formats — handled by refusing cleanly below the line (FA-3) via the
    read-a-policy mechanism (CF-03).

- **OQ-C — Branch/tag layout policy.** OQ-B settles the shape: **derived `RefRecord` reconstruction ships
  in M3**, under the standard `trunk`/`branches`/`tags` policy, **off by default**, always `Derived`, always
  with the immutability caveat on tags, and unresolvable layouts imported-with-loud-derived-record (not
  refused). **Still open (handoff-level, not owner-gated):** how configurable the policy should be — custom
  roots, nested layouts, per-project trunks. *Leaning:* the standard policy plus a simple configurable root
  set for M3; richer layout grammars deferred until a repository needs one.

- **OQ-D — Custom (user-defined) properties.** Drop-with-record for M3 (leaning — do not grow the frozen IR
  speculatively, D-9), or carry them in an IR sidecar? Defer carrying until a consumer exists.

- **OQ-E — Mergeinfo beyond dropping.** Confirmed: dropped-with-record / `AdvisoryUnreliable`, never
  ancestry (SRC-S2). Is there ever a case to surface it as a *`Derived` merge hint* (as renames are surfaced
  derived)? *Leaning: no for M3* — SVN merge tracking is too unreliable to even suggest; drop-with-record
  only.

- **OQ-F — Large repositories / streaming** (ties to RFC 003 OQ-B, RFC 004 OQ-D, RFC 005 OQ-E). SVN
  histories are linear but can be very long, and a dumpstream can be enormous. *Leaning:* defer; correctness
  and determinism first, streaming as a follow-up.

## Consequences

- Subversion decode → IR becomes buildable, delivering **M3** and giving the IR its **third source** — the
  first to stress the **derived** side (convention-based branches/tags) and the loss boundary (mergeinfo,
  byte-rewriting properties, externals) at scale.
- The **derived-marking discipline** becomes a shipped, checkable property on its archetype (FA-2): SVN
  branches/tags are carried as `Derived` over a `Stated` linear spine, convention-violating repositories are
  imported with the violation recorded as a derived judgment and named loudly (OQ-B), and the fidelity
  surface (FL-10) shows exactly what was reconstructed versus recorded.
- SVN follows the **RFC 004/005 decoder pattern** (isolated crate, all-`Stated` core mapping, opt-in derived
  layer beside the literal ops, read-a-policy floor, against-source verify), confirming the pattern
  generalizes to a source with **no DAG and no first-class refs** (IR-5) — with CVS (RFC 007) to follow.
- It is the **first post-freeze source**, and so the first real exercise of RFC 003 D-7's additive-only
  discipline — now **demonstrated, not merely asserted**: the D-9 additive-fit confirmation (below) finds
  SVN fits IR 1.0.0 with **zero** contract changes. The freeze is turned from a claim into a property its
  first post-freeze source exhibited.
- Now accepted, the immediate artifacts are the **`brygge-decode-svn` program-design handoff** and an
  **architect security review against `brygge-03`** — required because Tier D adds a **new untrusted-input
  parser** (the dumpstream) and a **subprocess posture** (`svnadmin`), even though it adds no gix-scale
  heavy dependency (GOVERNANCE security gate; RFC 009 D-6). The **D-9 additive-fit confirmation is done**
  ([`handoffs/006-subversion-decoder/d9-additive-fit-confirmation.md`](../handoffs/006-subversion-decoder/d9-additive-fit-confirmation.md)):
  zero IR changes, so the build spec assumes the frozen types as-is.
