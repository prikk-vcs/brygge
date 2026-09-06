# RFC 005 — Mercurial decoder

**Status.** Accepted (2026-09-06). Both owner-gated decisions are ruled: the **read tier is Tier 2 — the
pure-Rust revlog reader** (chosen over the `hg`-CLI subprocess to hold the clean/safe/secure/self-contained
line: no runtime dependency, no subprocess, `forbid(unsafe)` maximal, smallest trust surface — D-1/OQ-A),
and the **hg feature floor is ratified: refuse subrepos + largefiles + censored revisions** (D-4/OQ-B/OQ-D).
Tier 2 adds no gix-scale heavy dependency (only a small pure-Rust decompression codec), so acceptance
carries no separate security review — just the `deny.toml`/`cargo-audit` gate and a handoff note. Next
artifact: the `brygge-decode-hg` program-design handoff, then implementation toward M2.
**Tracks.** ROADMAP Phase A2 → milestone **M2 (Mercurial decode → IR)**. Track A — not prikk-gated
(decode stands alone, PU-1/PU-6). Realizes prikk RFC 113's decoder side for Mercurial, and is the
**IR's second-source validation — the RFC 003 D-7 contract-freeze precondition** (see D-8).
**Touches.** A new `brygge-decode-hg` crate (isolating whatever the read tier requires, RFC 009 D-1); the
`brygge decode hg` command (external design CL-01, FL-10); the against-source verify path (CL-04, VF-2);
possibly `deny.toml` / a runtime-dependency declaration (depending on the tier ruling); the threat model
(`T-2`/`INV-2`, revisited for the read tier).
**Requirements.** SRC-H1/H2/H3/H4; PR-2/3/4/5/7/8/9; HO-1/HO-2; NG-3/NG-5; IR-2/IR-5/IR-6; VF-1/VF-2/VF-5;
FA-1…FA-5; OQ-3. External design FL-10, CL-01/CL-04/CL-08, IX-02/03/04, CF-01/CF-03.

## Summary

Mercurial is the gradient's second source and is **epistemically closer to prikk than Git** (requirements
§0(a), SRC-Hg). Like Git it has content-addressed, atomic changesets and a real DAG, so content, ancestry,
and messages import faithfully **as claims** with the same discipline as RFC 004. The difference that
matters: **Mercurial records renames.** `hg mv`/`hg cp` (and commit-time copy detection the *source* chose
to record) write copy metadata into filelogs — a **source-stated fact** brygge carries as `Stated`, not a
guess it marks `Derived`. So an equivalent Mercurial import shows **fewer derived marks than a Git one**,
and that reduction is visible on the `inspect` and fidelity surfaces (FL-10) — a checkable consequence of
the source recording more (SRC-H2). Two things are genuinely hard and are where this RFC spends its care:
**how to read hg safely and deterministically** (the revlog-vs-CLI tier, RFC 009 D-2), and **hg's
structure with no clean prikk analogue** — named branches versus bookmarks, phases, obsolescence markers,
`.hgtags` — whose treatment is the owner's floor (OQ-3).

## The constraints that scope this design

- **Honour stated renames as stated** (SRC-H2/IR-2): where the source recorded a copy/rename, carry it as
  a `Stated` `RenameHint` — never re-derive it, never mark it `Derived`, never collapse the literal ops.
  This is the fidelity Mercurial uniquely offers over Git; losing it would flatten exactly what M2 exists
  to prove.
- **Add no precision hg did not have** (NG-5): where hg did *not* record a rename (a plain remove+add),
  carry delete+create as `Stated`, and mark a rename `Derived` only if brygge chooses to infer one — off
  by default, same as Git (RFC 004 D-3 / OQ-A).
- **Read without executing source-provided code, and deterministically** (RFC 009 D-4/INV-2/INV-3,
  `VF-1`): no hg extensions, no hooks, no network, no working-copy/dirstate mutation; same one-mechanism
  as RFC 004 D-6.
- **The heavy read surface stays isolated and license-clean** (RFC 009 D-1/D-5): whatever the tier,
  `brygge-ir` and `verify --internal` link none of it.
- **The IR must hold Mercurial without a breaking change** (IR-5/IR-6, RFC 003 D-7): if hg needs a new
  field, that is a *pre-freeze* contract event, stated — see D-8.

## Decisions

- **D-1 — A new `brygge-decode-hg` crate; nothing else in the workspace reads hg.** Per RFC 009 D-1 the
  crate is the only place the hg read path (a revlog reader and/or an `hg`-subprocess driver) lives;
  `brygge-ir`, the encoders, and `verify --internal` build and run without it (the RFC 009 D-7 isolation
  property, already tested for Git, generalizes). It exposes one narrow function — read a repository path,
  produce a `brygge_ir::Ir` (or a typed error). **Read tier: Tier 2 — a pure-Rust revlog reader
  (owner-ruled 2026-09-06).** brygge reads hg's on-disk revlog/filelog/manifest format directly, in
  process: no external `hg` binary, no subprocess, no runtime dependency, `forbid(unsafe)` stays maximal
  — the smallest, most self-contained trust surface, chosen to hold the clean/safe/secure line over the
  faster-to-build `hg`-CLI alternative. The one unavoidable dependency is **decompression** (revlogs are
  zlib-compressed, some zstd): a **pure-Rust** codec (e.g. `miniz_oxide`/`flate2` with the Rust backend,
  and a pure-Rust zstd if needed) — small and license-clean, **not** a gix-scale heavy dependency, so
  acceptance needs no separate security review, only the usual `deny.toml`/`cargo-audit` gate and a note
  in the handoff. The revlog reader is **bounds-checked and panic-free** on malformed input (untrusted
  source, T-2/INV-2), in brygge's hand-rolled-codec spirit.

- **D-2 — The object → IR mapping, mostly *Stated*.** Mirroring RFC 004 D-2:
  - **Changeset → `ChangeAtom`** with `status = Stated`. `parents` are the IR `AtomId`s of the
    changeset's parents (hg changesets have 1 or 2 parents; order preserved, PR-2). The changeset's own
    node id, the repository identity, and any commit signature go into `SourceIdentity` opaquely
    (PR-4/SRC-H4); the `AtomId` is the IR's own hash, never the hg node id.
  - **Manifest diff (parent→changeset) → `PathOp`s**, all `Stated`: added path → `Add`, changed
    content/flags → `Modify`, removed → `Delete`, in canonical path order. A root changeset diffs against
    the empty manifest. hg file flags (executable, symlink) map to the IR mode the same way Git modes do.
  - **File content → content store** by `BlobId` (SHA-256 of the raw file bytes); ops reference blobs;
    dedup.
  - **Author / description / date → `MetadataClaims`** (claims, never verified, PR-3).
  - **Node id + signature → `SourceIdentity`**, opaque; shown `Unverifiable` (FS-04/NG-3/SRC-H4).

- **D-3 — Stated renames are `Stated`; this is the point of M2 (SRC-H2).** Where a filelog records a
  copy/rename, brygge carries the literal delete+add (or add-only for a copy that keeps its source) as
  `Stated`, **plus** a `RenameHint { from, to, status: Stated }` — the source's own assertion, marked as
  fact, not judgment. brygge does **not** re-derive or second-guess it. Where hg recorded nothing, brygge
  marks a rename `Derived` only under the same opt-in, always-marked discipline as Git (RFC 004 D-3). The
  IR already holds both epistemic states on `RenameHint.status` (IR-2), so an hg import that used `hg mv`
  throughout has **zero** derived rename marks where the equivalent Git import would have many — the
  fidelity surface shows the difference (FL-10, IX-02).

- **D-4 — hg structure → IR, without privileging one model; the floor is owner-gated (OQ-3).** The IR
  already carries the variants hg needs, which is the design working as intended (IR-6):
  - **named branches → `RefKind::NamedBranch`; bookmarks → `RefKind::Bookmark`** — two distinct IR ref
    kinds, so neither is privileged and neither is flattened into the other (SRC-H3). Multiple heads per
    named branch → multiple `RefRecord`s.
  - **phases** (`public`/`draft`/`secret`) → **dropped-with-record** (Representation) — local workflow
    state, not history (PR-7).
  - **obsolescence markers / evolve** → **dropped-with-record** (`LossClass::AdvisoryUnreliable`) — advisory
    metadata about rewritten changesets (PR-8); never promoted to ancestry.
  - **`.hgtags`** → carried as **file content** (`Stated`) like any tracked file; whether brygge *also*
    synthesizes tag `RefRecord`s from it (a derived interpretation) is **OQ-C**.
  - On encountering a feature it will not approximate, the decoder **refuses with a named reason** and the
    CL-08 floor outcome (FA-3), reading a floor policy (CF-03) rather than hardcoding scope. The floor,
    **owner-ratified 2026-09-06 (OQ-B/OQ-D)**: refuse **subrepos**, **largefiles/lfs** pointers, and
    **censored revisions**.

- **D-5 — The hg loss boundary (HO-2/PR-7/PR-8), every drop class-stated (PR-9).** Representation-class:
  revlog physical layout and delta chains, the dirstate and working copy, phase roots. Advisory-unreliable:
  obsolescence markers. Ref namespaces that are workflow rather than authored history are dropped-with-record
  as under RFC 004 (OQ-B parity). Nothing in the never-silently-omit class is dropped without a record.

- **D-6 — Determinism and the untrusted-read guardrails are one mechanism (VF-1 + INV-2, as RFC 004 D-6).**
  Read logical objects, not physical revlog packing, so the output does not depend on how the repository
  was stored. Take **no ambient input**: **tier 2** reads revlogs directly and consults no hg config;
  **tier 3** runs `hg` with an **empty `HGRCPATH`, no extensions, no hooks, no network, and no dirstate or
  working-copy mutation** (`hg` invoked read-only, e.g. `log`/`cat`/`debugdata` against the store) — so no
  source-provided code executes (INV-2/T-2) and the same inputs always produce byte-identical output.
  `import_time` stays provenance-only (RFC 003 D-4/ID-4). Re-running after a failure reproduces the result
  up to the failure point (FA-5).

- **D-7 — Against-source verification by the preserved hg node ids (VF-2), as RFC 004 D-7.** Because every
  atom carries its hg node id (D-2/PR-4), `brygge verify --against-source <hg-repo>` re-derives from the
  source with the artifact's recorded options and confirms correspondence, without trusting the earlier
  run. This mode links `brygge-decode-hg`; `verify --internal` still links no decoder (RFC 009 D-1).

- **D-8 — This RFC validates the IR cross-source (RFC 003 D-7 freeze precondition).** The freeze needs a
  second source to exercise the IR and show it holds without a breaking change. **Preliminary finding: it
  does.** Every hg concept above maps onto the *existing* contract 0.1.0 — `SourceKind::Hg`,
  `RefKind::Bookmark`, `RefKind::NamedBranch`, a `Stated` `RenameHint`, and `LossClass::AdvisoryUnreliable`
  all already exist (the requirements v0.2 and the model anticipated hg). So the expected outcome is **no
  contract change** — the strongest possible freeze evidence. If implementation surfaces a genuine gap, it
  is a **pre-freeze minor contract bump with a stated migration** (RFC 003 D-7), recorded here rather than
  slipped in — not a freeze blocker, but a fact the freeze decision must weigh.

## Open questions

- **OQ-A — The read tier** — **RESOLVED 2026-09-06: Tier 2, the pure-Rust revlog reader** (D-1). The
  owner chose it over the `hg`-CLI subprocess to keep the dependency and trust surface smallest and the
  read fully in-process, accepting the higher implementation effort as the cost of a clean/safe/
  self-contained decoder. The only dependency is a **pure-Rust decompression** codec for zlib/zstd
  revlog payloads — small and license-clean, not a gix-scale heavy dependency (no separate security
  review; the `deny.toml`/`cargo-audit` gate and a handoff note suffice).
- **OQ-B — The hg floor contents (OQ-3)** — **RESOLVED 2026-09-06 (owner-ratified):** **refuse** subrepos
  (hg's submodule analogue — parity with the Git submodule floor, OQ-D), **largefiles/lfs** pointers, and
  **censored revisions**, each with a named reason (FA-3), rather than approximate them — the clean/safe/
  robust small-surface line. Everything core (changesets, DAG, stated renames, named branches, bookmarks,
  `.hgtags`-as-content) is carried or dropped-with-record per D-4. A later OQ-3 revision may move an item;
  the read-a-policy mechanism (CF-03) implements whatever line is set.
- **OQ-C — `.hgtags` beyond content.** Carry `.hgtags` as file content only (faithful, `Stated`), or
  *also* synthesize tag `RefRecord`s from parsing it (a `Derived` interpretation of a versioned file)?
  *Leaning:* content-only for M2; derived tag-ref synthesis deferred until there is a consumer for it.
- **OQ-D — Subrepos** — **RESOLVED 2026-09-06 (owner-ratified, with OQ-B):** refuse with a named reason,
  as the Git submodule analogue (RFC 004 D-4 parity).
- **OQ-E — Large repositories / streaming** (ties to RFC 003 OQ-B and RFC 004 OQ-D). *Leaning:* defer;
  correctness and determinism first.

## Consequences

- Mercurial decode → IR becomes buildable, delivering **M2** and giving the IR its **second source** — the
  concrete evidence the RFC 003 D-7 freeze needs (D-8).
- The **stated-rename advantage** becomes a shipped, checkable property: hg imports carry source-recorded
  renames as `Stated`, and the fidelity surface shows fewer derived marks than Git (SRC-H2/FL-10).
- hg follows the **RFC 004 decoder pattern** (isolated crate, all-*Stated* mapping, opt-in derived
  inference beside literal ops, read-a-policy floor, against-source verify), confirming the pattern
  generalizes across sources (IR-5) — with SVN (RFC 006) and CVS (RFC 007) to follow.
- On acceptance, the immediate artifacts are the **`brygge-decode-hg` program-design handoff** and — if
  the tier ruling adopts a heavy Rust dependency or the subprocess posture — an **architect security
  review** (RFC 009 D-6), as gix required; then the implementation toward M2.
