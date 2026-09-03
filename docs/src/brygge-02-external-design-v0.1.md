# brygge — External Design Specification

| | |
|---|---|
| Document | brygge External Design (black-box view) |
| Version | v0.2 (draft for review) |
| Date | 2026-09-03 |
| Inputs | brygge Requirements v0.2 (`brygge-01-requirements-spec-v0.1.md`) — cited as PU/NG/PR/HO/VF/ID/FA/BN/IR/UD/OQ; RFC 113 (import contract); prikk reality (0.27.1 audit; core now at 0.28 — the prikk-side gates are unchanged in kind and remain owner-open); project rules |
| Scope | WHAT brygge exposes at its boundaries — its command surface, the IR's external contract, the fidelity and provenance outputs, and the interaction flows — for **the parts that can be designed before the owner's open questions are settled.** The requirements are explicit that OQ-1…OQ-3 gate per-source encoder design; this document honours that by designing the pipeline, the IR contract, and the honesty surface now, and **stopping** at each gated point (§8). |
| Not | internal architecture, the IR's byte schema (the requirements forbid a schema), APIs, or code. |
| ID scheme | `BD-` boundary · `AC-` actor · `CL-` command surface · `IX-` IR external contract · `FS-` fidelity/honesty surface · `PX-` provenance-to-target interface · `CF-` configuration · `FL-` flow · `CT-` external data contract · `OP-` operational behaviour · `GATED-` a surface that cannot be designed until an owner question is answered. (`DC-` avoided — it collides with prikk's RFC numbering.) |

Design stance carried from requirements: **facts derive, judgment is authored, the join is checked** — so every user-visible surface makes the derived-vs-stated distinction inescapable (HO-1), makes loss legible (HO-2), and never lets honesty be turned off (HO-5). Where a Git mental model expects "just import it," the surface redirects to "decode, review fidelity, then encode a proposal."

---

## 1. System boundary & actors

### 1.1 Boundary (BD-…)

```
   [source repos]           [source-system libraries]        [the operator + reviewers]
   Git / SVN / CVS  ─read→   gix/libgit2, SVN, CVS      ┌──── runs, reviews, decides ────┐
        │                    (brygge's heavy deps)      │                                │
        ▼                            ▼                  ▼                                ▼
  ┌──────────────────────── brygge process ─────────────────────────────────────────────┐
  │  DECODE (per source) ─▶ the IR (faithfulness-with-provenance) ─▶ ENCODE (per target)  │
  │        every atom marked stated-vs-derived (IR-2); source ids preserved opaque (IR-3) │
  │  ───────────────────────────────────────────────────────────────────────────────────│
  │  FIDELITY SURFACE (HO-4): what was preserved / derived / dropped / refused            │
  └───────────────────────────────────────┬───────────────────────────────────────────────┘
                                           ▼
                           [target proposal + provenance]  ──▶  target system (prikk, or another)
                                                                 decides admission / trust / seal
                                                                 (BN-2 — NOT brygge)
```

- **BD-01 — Inside brygge:** the per-source decoders, the IR, the per-target encoders, and the honesty machinery that spans all three (derived-marking, loss-boundary recording, the fidelity summary). brygge is responsible for reading the source correctly, representing it faithfully-with-provenance, translating it, and making every derivation and loss legible (BN-1).
- **BD-02 — Outside brygge:** the source repositories; the heavy source-system libraries brygge links (`gix`/`libgit2`, SVN, CVS — PU-5); the operating system; the target system (prikk or another), which owns admission, trust, verification, sealing, and storage (BN-2); and the humans.
- **BD-03 — The dependency-weight boundary is a hard external property** (BN-5): brygge may link whatever it needs to read a source, but **nothing it emits may require the target to link any of those.** A prikk repository verifies a brygge import using only prikk's own five-crate surface; the import's checkability (VF-3) never routes through a brygge dependency. This is externally observable: a consumer can confirm the target proposal and the IR are consumable with none of brygge's decoder dependencies present.
- **BD-04 — brygge never mutates the target** (BN-4): it produces files; it does not admit, seal, merge, or reconcile. A re-import is a fresh translation (ID-2); what the target does with it is the target's.
- **BD-05 — Two separable halves, separable in the boundary** (PU-1/PU-3): decode → IR is complete and useful without any encoder; the IR is a durable, inspectable artifact between the halves, not an internal handoff. A second target's encoder is a consumer of the IR at this boundary, not a modification of brygge.

### 1.2 Actors (AC-…)

brygge has no accounts; roles are what a person is *doing*, and map to which surfaces they touch.

| ID | Actor | Does | Touches |
|---|---|---|---|
| AC-01 | **Migrator** | runs decode/encode over their own source | CL decode/encode, CF parameters, FS summary |
| AC-02 | **Fidelity reviewer** | reads what an import preserved/derived/dropped before trusting it | FS surface, IR inspection (CL inspect), VF-3 internal checks |
| AC-03 | **Third-party verifier** | holds the *original source* and checks the import corresponds to it | VF-2 round-check (CL verify --against-source), PX provenance |
| AC-04 | **Target maintainer** | decides whether to admit/seal the proposal — **outside brygge** (BN-2) | receives PX provenance + the proposal; acts in the *target's* tool, not brygge |

Note AC-04 is listed to mark the boundary: brygge's obligation to them is provenance sufficient to decide (BN-3, PX-*), not a decision.

---

## 2. External interfaces

### 2.1 Command surface (CL-…)

The command surface makes the **decode/encode separation** visible and the **honesty outputs** unavoidable. It is a tool surface, not a target's VCS surface — brygge issues no VCS verbs against the target (BD-04).

- **CL-01 — `brygge decode <source-kind> <source-location> --ir <out>`** — read a source into an IR artifact. `<source-kind>` ∈ {`git`,`hg`,`svn`,`cvs`} today, each with its own honestly-scoped support (SRC-*); the set is **open by design** — a new source is a new `<source-kind>` value backed by a new decoder against the shared IR obligations (PU-3, IR-5), not a brygge redesign. Decode stands alone (PU-1) and needs no target; it is the half brygge stabilizes first (PU-6). Records the inference parameters it used into the IR (PR-5), and always writes a fidelity record for the decode half (FS-01).
- **CL-02 — `brygge inspect --ir <file>`** — read an IR artifact and report, for AC-02: its atoms, each atom's **epistemic status** (stated-by-source vs derived-by-decoder, IR-2), the opaque source identifiers preserved (IR-3), and the loss boundary (IR-4). Read-only; the reviewer's microscope.
- **CL-03 — `brygge encode <target-kind> --ir <file> --out <proposal>`** — build a target proposal from an IR. `<target-kind>` starts at `prikk`; the command is designed so a second target is a new `<target-kind>` value implemented against the IR alone (PU-3, IR-5), not a brygge change. For prikk today it emits a **reviewable proposal** — labelled, unsealed, authorship `Unverifiable` — never sealed history (§8 GATED-1/2/3). Always writes the encode-half fidelity record (FS-01).
- **CL-04 — `brygge verify`** — two modes, matching the two checkable meanings of "faithful":
  - `--internal --import <proposal-or-ir>` → the VF-3 check any reader can run without the source: every derived record marked, loss boundary stated, authorship `Unverifiable`, content/ancestry internally consistent, provenance names its source.
  - `--against-source <source-location> --import <proposal-or-ir>` → the VF-2 round-check for AC-03: using the opaquely-preserved source identifiers (PR-4), confirm the import corresponds atom-by-atom to the original source, **without trusting brygge**.
- **CL-05 — `brygge summary --import <proposal-or-ir>`** — reproduce the fidelity summary (FS-01) from the objects themselves, demonstrating HO-4's "recoverable by a later reader without brygge's run log." That this reproduces byte-for-byte what the run printed is the externally-testable form of "the summary travels with the import."
- **CL-06 — `--version`, `--help`, per-command `--help`.** brygge is a tool, so unlike stikk it *does* own its full command vocabulary and help surface.
- **CL-07 — Machine-readable output.** Every command that reports (`inspect`, `verify`, `summary`, and the tail of `decode`/`encode`) offers a stable machine-readable form so a migration can be scripted and gated in CI; the human form is the default. (The requirements forbid a schema for the *IR*; this is the *tool's* report format, a separate, versioned contract — CT-04.)
- **CL-08 — Exit codes carry the outcome class** (a lesson taken from stikk's audit of prikk's coarse 0/1): `0` clean; a distinct non-zero for *import completed with recorded loss* (the normal, honest outcome — not an error, but not silent either); a distinct non-zero for *refused a source feature below the floor* (FA-3); another for *source violated its own conventions and was not resolved* (FA-2); another for *partial/interrupted* (FA-1); and a generic failure. A CI gate can therefore distinguish "faithful within stated limits" from "hit the floor" without parsing prose.

### 2.2 The IR's external contract (IX-…)

The requirements forbid a schema (they are requirements on the IR, IR-1…IR-6). The external design states the IR's **black-box contract** — what any consumer may rely on — without defining bytes. That contract is the product boundary of PU-3.

- **IX-01 — The IR is a durable, inspectable artifact**, not an internal handoff (BD-05). It can be written by `decode`, read by `inspect`/`verify`/`encode`, kept, diffed, and consumed by a foreign encoder.
- **IX-02 — Every atom exposes its epistemic status** (IR-2): a consumer can always ask "did the source state this, or did a decoder derive it?" and get an answer, per atom, with the governing parameters when derived (PR-5). An IR that cannot answer is non-conformant.
- **IX-03 — Source identifiers and signatures are first-class, opaque content** (IR-3, PR-4): a consumer can read a Git SHA, a GPG signature, an SVN revision number, a CVS revision tag back out unchanged — brygge neither interprets them as target-meaningful nor discards them.
- **IX-04 — The loss boundary is queryable** (IR-4): a consumer can ask, per import, what *class* of information the decoder dropped and what it derived — the shape of the loss without re-deriving it.
- **IX-05 — The IR privileges no target's identity model** (IR-6): it does not bake in prikk's node identity; a snapshot-based target encoder finds a snapshot it can use, and a node-identity target encoder finds the source facts plus the marked places where it must author inference itself. Identity inference is the *encoder's* authored judgment, marked as such — never pre-decided in the IR.
- **IX-06 — The IR's answers to "what is a record / preserved / omitted" are shared across decoders** (IR-5): a Git IR and a CVS IR are comparable, so "how faithful was this import?" has a cross-source answer. This is what makes the IR a reusable abstraction rather than three private formats.
- **IX-07 — The IR is versioned and its version is legible** (supports ID-1/VF-1): a consumer knows which IR contract it is reading, and a re-decode under the same brygge version produces the same IR.

### 2.3 The fidelity & honesty surface (FS-…)

- **FS-01 — Every run emits a fidelity record, and it is unskippable** (HO-4, HO-5): at the end of `decode` and of `encode`, brygge states what was preserved, what was derived (with confidence/parameters), what was dropped (by class), and what was refused. No flag suppresses it (HO-5); verbosity may vary, existence may not.
- **FS-02 — The summary is recoverable from the objects, not just printed** (HO-4): `brygge summary` (CL-05) reconstructs it from the IR/proposal alone. This is the external proof that honesty "travels with the import" and cannot be lost by deleting a log.
- **FS-03 — Derived records are visibly marked wherever they appear** (HO-1): in `inspect`, in the summary, and in the target proposal, an inferred rename / reconstructed CVS changeset / inferred SVN branch reads as *derived by brygge*, distinct from a source-stated fact, without the reader re-running the heuristic.
- **FS-04 — Authorship is shown `Unverifiable`, never dressed up** (HO-3, NG-3): nowhere in any brygge surface does imported authorship read as sound, verified, or native. A GPG-signed Git commit is shown with its signature *preserved* and *verifying nothing in the target* — the two are never conflated (VF-4).
- **FS-05 — Per-source faithfulness is stated before the run, not after** (VF-5): `decode`/`encode` for a given source declare, up front, what faithfulness *can* mean for that source and what it cannot — most sharply for CVS (FS-06).
- **FS-06 — The CVS surface states its verdict before it runs** (SRC-C3): a CVS decode announces that a faithful-in-VF-2's-sense import is *not achievable* (no atomic source atom to check against), that the deliverable is a lossy, explicitly-labelled reconstruction, and that changeset grouping is brygge's derived judgment — so "faithful CVS import" is never a promise the surface implied and broke.

### 2.4 The provenance-to-target interface (PX-…)

This is brygge's obligation *to the target* (BN-3) — the interface across the boundary at which the target makes its own admission/trust/seal decisions.

- **PX-01 — Every proposal carries provenance** (PR-6): source repository identity, source atom identifiers, brygge version, decoder version, target and version, and the inference parameters (PR-5). Enough for the target to make its decisions and for a third party to run VF-2.
- **PX-02 — Provenance is separable from the history it describes** (RFC 113 §4.1's shape): a *statement about* the imported history, not baked into the history's atoms — so the target can carry it as its own kind of object (for prikk, an `Attestation`) rather than altering what a patch/commit *is*.
- **PX-03 — The provenance carries the honesty boundary** (HO-2 → BN-3): the target (and a bundle receiver downstream) can read what class of information was dropped and what was derived, so honesty survives the hand-off, not just the run.
- **PX-04 — The exact prikk provenance object shape is GATED** on prikk (§8 GATED-1): PX-01…PX-03 state *what* provenance must convey; the *form* it takes in prikk is prikk's to define (UD-1), and brygge targets whatever prikk settles.

### 2.5 Configuration (CF-…)

- **CF-01 — Inference parameters are configurable and recorded** (PR-5): similarity thresholds (Git renames), clustering windows (CVS changesets), branch conventions (SVN) may be set; whatever is set is written into the IR/provenance, so the output stays reproducible (VF-1) and the judgment reviewable. A parameter that changed the output but was not recorded would break determinism's checkability — forbidden.
- **CF-02 — Honesty controls are not configurable** (HO-5): no flag turns off derived-marking, the loss boundary, `Unverifiable`, or the fidelity summary. Configuration tunes *inference*, never *honesty*.
- **CF-03 — The source-side floor is configuration the owner sets, not the migrator** (OQ-3): which source features are refused rather than approximated is product scope. brygge exposes the floor as a declared policy it enforces (FA-3), and the migrator sees refusals, not a knob to lower the floor. The floor's *contents* are GATED (§8 GATED-4).

---

## 3. User interaction flows (FL-…)

Numbered user-action → system-response. Each cites the requirement it realizes.

- **FL-01 — Decode a Git repository (AC-01).** 1. `brygge decode git <path> --ir out.ir`. 2. brygge states, up front, Git's faithfulness scope (FS-05) and any features it will refuse under the floor (CF-03). 3. It reads content, ancestry, messages as claims (PR-1/2/3), preserves commit SHAs and GPG signatures opaquely (PR-4), and **marks every inferred rename as derived** with its similarity parameters (HO-1/FS-03). 4. It writes the IR and prints the decode fidelity record (FS-01): preserved / derived / dropped / refused. 5. Exit code carries the outcome class (CL-08).
- **FL-02 — Inspect the IR before trusting it (AC-02).** 1. `brygge inspect --ir out.ir`. 2. brygge lists atoms with each one's epistemic status (IX-02) — a reviewer sees exactly which records are source-stated and which are brygge-derived, and the parameters behind each derivation. 3. The reviewer reads the loss boundary (IX-04): what class was dropped, what was derived. No target is involved; this is fidelity review, not migration.
- **FL-03 — Encode a prikk proposal and read its fidelity (AC-01).** 1. `brygge encode prikk --ir out.ir --out proposal`. 2. brygge builds a prikk **proposal** — labelled, unsealed, authorship `Unverifiable` (FS-04) — because sealing/admission is the target's and the prikk import surface is not yet built (§8). 3. It attaches provenance (PX-01) in the shape prikk will settle (PX-04). 4. It prints the encode fidelity record (FS-01) and states, in-band, that this is a proposal the target has not admitted (FA-4). 
- **FL-04 — Verify internally, then against the source (AC-02 then AC-03).** 1. `brygge verify --internal --import proposal` confirms every derived record is marked, the loss boundary is stated, authorship is `Unverifiable`, and provenance names its source (VF-3) — provable with no source present. 2. A third party runs `brygge verify --against-source <original-git> --import proposal`; using the preserved SHAs (PR-4), brygge confirms the import corresponds to the original atom-by-atom **without trusting brygge** (VF-2). 3. The report distinguishes "corresponds to source" (VF-2) from "internally honest" (VF-3) — never conflating them (VF-4).
- **FL-05 — Re-run and get identical output (AC-01, ID-1/VF-1).** 1. The migrator re-runs the same `decode`+`encode` with the same parameters and brygge version. 2. brygge produces the **same IR and the same proposal**. 3. If anything differs, brygge states the cause (new version, changed parameters, changed source) — "I ran it again and got different history" is never silent (ID-1). 4. For a content-addressed target, any field brygge cannot make deterministic (e.g., an import timestamp, if prikk's provenance object carries authoritative time) is named as a stated non-determinism, not left to silently perturb ids (ID-4, UD-4).
- **FL-06 — Hit a refused feature (AC-01, FA-3).** 1. A Git repo uses submodules / an octopus merge beyond the target's parent limit / grafts. 2. brygge **refuses** the feature with a named reason (not an approximation), tells the migrator exactly which feature and why, and exits with the "hit the floor" class (CL-08). 3. The migrator knows precisely who can migrate and who is told no — set by policy (CF-03), not guessed.
- **FL-07 — An SVN repo that violates its own convention (AC-01, FA-2, SRC-S1).** 1. `brygge decode svn …` finds a layout that breaks the `/trunk`/`/branches/x` convention branch identity depends on. 2. brygge does **not** silently pick an interpretation: it refuses, or it imports with the reconstructed branch recorded as a **derived judgment** the migrator must accept (HO-1). 3. Mergeinfo, being advisory and frequently wrong, is dropped-with-record or carried-as-advisory-and-labelled, never promoted to a real merge parent (PR-8/SRC-S2).
- **FL-08 — A CVS run surfaces its lossy reconstruction (AC-01, SRC-C3).** 1. `brygge decode cvs …` announces, *before running* (FS-06), that a faithful-in-VF-2's-sense import is not achievable and the deliverable is a lossy, explicitly-labelled reconstruction. 2. It preserves per-file content and history faithfully, and reconstructs changesets by clustering (with the window recorded as a parameter, PR-5). 3. Every reconstructed changeset is marked **derived** (HO-1); the fidelity summary makes the reconstruction's uncertainty prominent, not buried (FS-01). 4. The migrator was told the honest deliverable up front, so no promise was made and broken.
- **FL-09 — Interrupted import (AC-01, FA-1/FA-4/FA-5).** 1. A run stops mid-stream. 2. What was and was not imported is stated; the partial result is distinguishable from a complete one (FA-1). 3. Because admission is the target's (BN-2), brygge's safe failure is a clearly-incomplete-and-labelled proposal the target has not admitted — never a half-admitted history that looks whole (FA-4). 4. Re-running reproduces the same result up to the failure point (FA-5).
- **FL-10 — Decode a Mercurial repository (AC-01, SRC-H1…H4)** *(conceptually the second source, after FL-01).* 1. `brygge decode hg <path> --ir out.ir`. 2. brygge states hg's faithfulness scope (FS-05) and any floor refusals (CF-03). 3. Where the source **recorded** a rename (`hg mv`), brygge carries it as a **source-stated** fact — *not* marked derived (SRC-H2, IX-02); where it did not, brygge carries delete+create as stated, marking a rename **derived** only if it infers one (HO-1/FS-03). The inspect and fidelity surfaces therefore show hg imports with **fewer derived marks** than an equivalent Git import — a visible, checkable consequence of hg recording more. 4. Named branches and bookmarks are mapped to the IR's branch-identity notion without privileging one; phases and obsolescence markers are dropped-with-record as representation/advisory (PR-7/PR-8, HO-2); `.hgtags` history is preserved as content. 5. Fidelity record + outcome-class exit as FL-01.

---

## 4. Data contracts (external) (CT-…)

- **CT-01 — Inputs brygge accepts:** a source repository (Git/hg/SVN/CVS, by kind + location); an IR artifact (for `inspect`/`verify`/`encode`); inference parameters (CF-01); the declared source-side floor policy (CF-03, contents GATED). No target credentials, no network endpoints — brygge reads sources and writes files.
- **CT-02 — Outputs brygge produces:** the **IR artifact** (its external contract is IX-*, its byte schema deliberately undefined here); the **target proposal** (for prikk: labelled, unsealed, `Unverifiable`, with provenance); the **fidelity record/summary** (FS-01, reproducible via CL-05); and **verification reports** (VF-2/VF-3, CL-04). Every output is a file or a report; brygge writes nothing into the target's storage (BD-04, BN-2).
- **CT-03 — The provenance interface to the target** (PX-*): what brygge conveys across the boundary for the target's admission/trust/seal decisions. Its *content* is specified (PX-01…PX-03); its *prikk form* is GATED (PX-04, UD-1).
- **CT-04 — The tool's machine-readable report format** (CL-07) is a versioned contract, distinct from the IR: it is how a CI gate consumes `verify`/`summary`/outcome classes. Versioned so a migration pipeline can pin it. (This is a *report* schema, permitted; the *IR* schema remains the design phase's, not this document's.)
- **CT-05 — What a prikk repository must be able to assume about brygge output** (BN-5, the requirements' central constraint): that it is consumable and checkable with **only prikk's own five-crate surface** — no brygge dependency required to read the proposal, verify it internally (VF-3), or carry its provenance. This is the externally-testable form of "brygge carries the weight, prikk does not."

---

## 5. Operational behaviours (OP-…)

- **OP-01 — Determinism is an observable property, not a hope** (VF-1): the same inputs yield byte-identical IR and proposal; `verify --against-source` and a re-run are the two ways a user confirms it. Any non-determinism brygge cannot avoid is named (ID-4).
- **OP-02 — Large imports show progress and are cancellable** (a real migration is long): brygge reports progress and can be stopped; a stop is a clean partial (FA-1/FA-4), never an ambiguous target state.
- **OP-03 — Honesty is emitted continuously, not only at the end:** refusals (FA-3) and convention violations (FA-2) surface at the atom they occur on, so a long run does not hide a floor-hit until completion.
- **OP-04 — Error presentation names the class** (mirrors CL-08): *recorded loss* (normal, honest), *floor refusal*, *convention violation*, *partial/interrupted*, *internal failure* — each distinct, each actionable, none collapsed into a bare failure.
- **OP-05 — brygge leaves the target untouched on any exit** (BD-04, FA-4): success produces a proposal the target has not yet admitted; failure produces a labelled partial; neither writes into the target's storage.

---

## 6. Traceability (design items → requirements)

| Design area | Realizes |
|---|---|
| BD-01…05 · AC-01…04 | BN-1/2/4/5, PU-1/3/5, IR-6 |
| CL-01…08 | PU-1/2/3, FS/HO surface, VF-2/VF-3, FA-1/2/3, CL-08 ↔ audit lesson |
| IX-01…07 | IR-1…IR-6, PR-4, ID-1/VF-1 |
| FS-01…06 | HO-1…HO-5, NG-3, VF-4/VF-5, SRC-C3 |
| PX-01…04 | PR-6, BN-3, RFC 113 §4.1, UD-1 |
| CF-01…03 | PR-5, HO-5, OQ-3 |
| FL-01…09 | PU-1, PR-*, HO-1, VF-2/3/4, ID-1/4, FA-1…5, SRC-S1/S2/C3 |
| CT-01…05 | BN-2/5, IX-*, PX-*, CL-07 |
| OP-01…05 | VF-1, FA-1/4/5, ID-4 |

---

## 7. Design-set entry point (for the owner)

Per project-rules §Workflow. Two brygge documents exist:

1. `brygge-01-requirements-spec-v0.1.md` — what it must do / never do / decide (the contract).
2. `brygge-02-external-design-v0.1.md` — this document: the black-box surface for what can be designed now.

The load-bearing design decisions this document commits: the **decode/IR/encode split is visible in the command surface** (CL-01/03) and the IR is a durable, inspectable artifact (IX-01), so decode and a second target's encoder are real products, not internal steps; **honesty is a surface property, not a report** (FS-*), unskippable and recoverable from the objects; and **brygge's dependency weight stops at its boundary** (BD-03/CT-05), the externally-testable form of the constraint that created the project.

---

## 8. What cannot be externally designed yet — GATED surfaces (GATED-…)

The requirements are explicit that OQ-1…OQ-3 gate per-source design (RFC 113 §4a). This document designs up to those gates and **stops**, rather than inventing a surface that presumes an answer. Each gate names what it blocks and what brygge does in the meantime.

| ID | Gated surface | Blocked on | brygge's interim behaviour |
|---|---|---|---|
| **GATED-1** | The exact prikk **provenance object** shape the `encode prikk` proposal attaches (PX-04) | UD-1 — prikk's `Attestation` is audit-shaped and never constructed; import use is a format change (RFC 113 §4.1) | PX-01…PX-03 fix the provenance *content*; the prikk *form* is targeted once prikk defines it. The proposal carries provenance in a clearly-labelled interim form, not a claimed-final prikk object |
| **GATED-2** | Whether the prikk proposal uses an **`Import` block kind** or `Normal` blocks | UD-2 — `Import` kind is defined but **refused** by prikk's validator | The proposal is structured so either ruling is targetable; it does not emit an `Import` block prikk would reject today |
| **GATED-3** | Whether the proposal can become **sealed** prikk history, and the `encode` surface for that | UD-3 / OQ-2 — sealing imports is an open owner question | `encode prikk` produces an **unsealed, `Unverifiable` proposal only** (FL-03); no seal path is exposed; the surface states this is the honest ceiling until the owner rules |
| **GATED-4** | The **contents** of the per-source floor (which features are refused) | OQ-3 — the floor is product scope, the owner's | CF-03 exposes the floor *mechanism* (declared policy, enforced by refusal FA-3); the specific refused-feature lists per source are filled in when the owner sets them |
| **GATED-5** | What the target's **`verify` reports** about an import, which the FS/PX surface must align with | OQ-4 (downstream of OQ-1) | FS-04/VF-4 keep the two claims distinct regardless of the answer; the surface adapts to whichever prikk specifies |
| **GATED-6** | What, if anything, the **importer signs** — and thus whether `encode` has a signing step at all | OQ-1 / DC-35 — who may sign what | No signing surface is designed; "I imported this" as a signable claim is left unbuilt until the owner rules who may assert it (NG-6 keeps brygge out of authoring authority) |

**Sequencing consequence** (RFC 113 §4a, §5; requirements §7 gradient): the IR obligations (IX-*) and the honesty/verification surface (FS-*, VF-*) can be internally designed next, source by source, in the order **Git → Mercurial → SVN → CVS** — the decode/IR half, which depends on no prikk decision and is what brygge **stabilizes first** (PU-6). The prikk encode surface (CL-03 for `prikk`) cannot be finalized past a reviewable proposal until GATED-1/2/3 land; the per-source floors cannot be finalized until GATED-4 (for hg that includes phases/obsmarkers and the named-branch-vs-bookmark mapping). This document deliberately leaves those open rather than guess — a confident wrong external surface would be worse than a stated gate (the requirements' own rule).

*End of External Design v0.2. Next internal design work should start where no gate blocks it: the IR's internal representation satisfying IX-*/IR-*, then the decoders in gradient order (Git → Mercurial → SVN → CVS, CVS last and permitted to be lossy). This decode/IR half is the near-term stabilization target (PU-6). The prikk encoder past a reviewable proposal waits on the §8 gates (GATED-1…6, i.e. prikk's UD-1…UD-3 and the owner's OQ-1…OQ-3).*
