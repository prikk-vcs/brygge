# RFC 006 — Subversion decoder

**Status.** **PROPOSED 2026-09-08.** Not accepted; nothing may be implemented from it. Its owner-gated
questions — the **read tier** (OQ-A) above all, and the **SVN floor** (OQ-B) — are unresolved, and per
`GOVERNANCE.md` the read tier is an owner decision (RFC 009: the decoder's dependency surface is the
project's defining risk). This RFC states the design and hands the owner the rulings to make.

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

- **D-1 — A new `brygge-decode-svn` crate; nothing else in the workspace reads SVN. The read tier is
  owner-gated (OQ-A) and is the central decision of this RFC.** Per RFC 009 D-1 the crate is the only place
  the SVN read path lives; the RFC 009 D-7 isolation property (already tested for Git and hg) generalizes.
  It exposes one narrow function — read a Subversion source, produce a `brygge_ir::Ir` (or a typed error).
  **The tier choice is genuinely harder than hg's, and the hg answer does not transfer.** For Mercurial,
  a pure-Rust on-disk reader (Tier 2) was the *clean* choice — revlog is a single, tractable format. For
  Subversion, a pure-Rust on-disk reader is the *reckless* choice: the native store is **FSFS** (a complex,
  multi-version format) with a legacy **BDB** backend that is effectively a Berkeley-DB C artifact — a large,
  versioned, untrusted parser, or an FFI dependency, exactly the surface RFC 009 exists to contain. The
  honest clean-equivalent for SVN is not the on-disk format but the **dumpstream** (§ OQ-A):

  > **Architect recommendation (OQ-A leaning): Tier D — a pure-Rust parser of the SVN *dumpstream*, fed by
  > a user-supplied dumpfile or a read-only local `svnadmin dump`.** The dumpstream is a documented, stable,
  > framed format; parsing it is pure Rust, bounds-checked and panic-free on malformed input (untrusted —
  > a dumpfile may be attacker-controlled, T-2/INV-2). No FFI, no linked SVN library, **no network** (a
  > local `svnadmin dump` reads a local repository; `svnrdump` over a URL is **excluded** by INV-3). Any C
  > lives only in the `svnadmin` *producer* — a read-only subprocess outside brygge's trust boundary and
  > absent from the produced artifact (BN-5) — or is avoided entirely when the user supplies the dumpfile.

  The owner rules; the alternatives and their costs are laid out in OQ-A. The one dependency any tier may
  add (a decompression codec, or a subprocess driver) stays pure-Rust and license-clean where possible, and
  a heavy or FFI dependency triggers the RFC 009 D-6 architect security review before acceptance.

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
    - a repository whose layout the active policy **cannot resolve** is **refused with a named reason**
      (FA-2/FA-3, CL-08) or imported with the violation recorded as a derived judgment the user must accept —
      the owner's floor sets which (OQ-B/OQ-C);
    - a **`Derived` tag carries the recorded caveat that SVN tags are not guaranteed immutable** (they are
      ordinary directories and may have been committed to after creation) — an honesty fact 1.0.0 must be
      able to express (D-9 candidate).
    brygge **never** promotes a derived branch/tag to `Stated`, and never fabricates a branch a strict
    reading of the copies does not support. A false branch reconstruction is precisely the manufactured
    verification the whole project forbids (INV-1) — the same rule as Git rename inference (RFC 004 D-3).

- **D-5 — SVN properties → mapped, dropped-with-record, or refused; every drop class-stated (PR-9).**
  - `svn:executable` → IR executable mode (`Stated`); `svn:special` → IR symlink (`Stated`).
  - `svn:mergeinfo` → **`AdvisoryUnreliable`, dropped-with-record, never a merge parent** (SRC-S2/PR-8).
  - `svn:externals` → **refused with a named reason** (or dropped-with-record, per the floor): it references
    other repositories/paths — a network and trust surface, the SVN analogue of submodules/subrepos
    (INV-3; parallels RFC 004/005 submodule/subrepo floors). Owner-gated (OQ-B).
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
  freeze's first exercise under discipline.** Most of SVN maps onto the *existing* 1.0.0 contract, which is
  the design working as intended (IR-6): a linear `Stated` spine, `Stated` copies via `RenameHint`,
  `Derived` `RefRecord`s for reconstructed branches, `LossClass::AdvisoryUnreliable` for mergeinfo, and a new
  `SourceKind::Svn`. **Preliminary finding: no *breaking* change is required.** The candidate *additive*
  needs — surfaced now for owner review, not slipped in — are:
  1. a way to record that a `Derived` branch/tag came from **path convention** with the convention as its
     parameter (may already be expressible via the existing derived-parameter field, PR-5 — to confirm);
  2. a way to record the **"tag not guaranteed immutable"** caveat on a derived tag (D-4);
  3. possibly a `RefKind` or ref-attribute distinguishing a convention-derived branch from a first-class one
     (may already fold into `RefKind` + `Derived` status — to confirm).

  Each, if needed, is an **additive** optional field or enum variant an existing 1.0.0 reader ignores (or
  refuses under the read gate) — permitted within major 1. **Anything that would require a 1.0.0 reader to
  change to stay correct is out of scope for M3 and is deferred, not done** (RFC 003 D-7). Whether items 1–3
  need *any* new field, or are already expressible, is settled during the handoff against the shipped
  `brygge-ir`, and recorded here.

## Open questions

- **OQ-A — The read tier (owner-gated; the central decision).** Options, with costs:
  - **Tier D — dumpstream parser (pure Rust), fed by a user-supplied dumpfile or a read-only local
    `svnadmin dump`** — *architect recommendation* (D-1). Smallest pure-Rust honest surface; no FFI, no
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
    Not recommended.
  The owner rules; RFC 009 and GOVERNANCE make this an owner decision because the decoder's dependency
  surface is the project's defining risk.

- **OQ-B — The SVN floor contents (OQ-3, owner-gated).** Which features are **refused** rather than
  approximated. Candidates: `svn:externals` (reaches other repositories, INV-3); layouts the active
  convention policy cannot resolve (SRC-S1/FA-2 — refuse vs import-with-loud-derived-record); BDB-backend
  repositories if the tier cannot read them; very old or partial dump formats. The owner sets the line;
  brygge implements it via the read-a-policy mechanism (CF-03) and refuses cleanly below it (FA-3).

- **OQ-C — Branch/tag layout policy.** Default to standard `trunk`/`branches`/`tags`; how configurable
  should the policy be (custom roots, nested layouts, per-project trunks)? And does derived branch/tag
  **`RefRecord` synthesis ship in M3**, or does M3 carry only the stated directory-copy truth with
  reconstruction deferred (parallel to hg OQ-C `.hgtags`)? *Leaning:* ship reconstruction under the standard
  policy, **off by default**, always `Derived` and always with the immutability caveat on tags; refuse
  unresolvable layouts per OQ-B.

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
  refused or flagged, and the fidelity surface (FL-10) shows exactly what was reconstructed versus recorded.
- SVN follows the **RFC 004/005 decoder pattern** (isolated crate, all-`Stated` core mapping, opt-in derived
  layer beside the literal ops, read-a-policy floor, against-source verify), confirming the pattern
  generalizes to a source with **no DAG and no first-class refs** (IR-5) — with CVS (RFC 007) to follow.
- It is the **first post-freeze source**, and so the first real exercise of RFC 003 D-7's additive-only
  discipline: the handoff confirms SVN fits IR 1.0.0 with at most additive changes (D-9), and any construct
  that would break a 1.0.0 reader is deferred — turning the freeze from a claim into a demonstrated property.
- On acceptance, the immediate artifacts are the **`brygge-decode-svn` program-design handoff**, an
  **architect security review if the tier ruling adopts a heavy or FFI dependency or a subprocess posture**
  (RFC 009 D-6), and the D-9 additive-fit confirmation against the shipped `brygge-ir`; then implementation
  toward M3.
