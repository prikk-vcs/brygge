# brygge — Requirements Specification

| | |
|---|---|
| Document | brygge Requirements (what it must do, must never do, and must decide) |
| Version | v0.2 (draft for review) |
| Date | 2026-09-03 |
| Basis | RFC 113 (History import foundations, Proposed) as the governing import contract; prikk reality per the 2026-08-31 audit (then 0.27.1; the live core has since moved to 0.28 — the prikk-side dependencies §11 are to be **re-verified against the current prikk when encoder design begins**, and none of them gate the decode/IR half); project rules in `.git-exclude/rules/` |
| Not | a design, a schema, an API, or code. Where a decision belongs to a human, it is named in §11/§12 and left there. |
| ID scheme | `PU-` purpose · `NG-` non-goal · `PR-` preserve rule · `HO-` honesty rule · `VF-` verification · `ID-` idempotence · `FA-` failure · `BN-` boundary · `IR-` IR obligation · `SRC-` per-source · `UD-` prikk-side dependency · `OQ-` open question |

**brygge** (Norwegian: *wharf* — where cargo is landed) carries version-control history **out of** existing systems (Git, Mercurial, Subversion, CVS — and, by design, others) and **into** a different one, through an **intermediate representation (IR)** that belongs to no particular system. The pipeline is two deliberately separated halves: **decode** a source into the IR; **encode** a target from the IR. The first target is prikk; the IR and decoders are meant to be reusable so other version-control projects can write their own encoders and get import "for free," and so a new *source* is a new decoder against the same IR, not a redesign.

**Two halves, versioned separately (v0.2).** The **decode → IR half depends on the source systems, which are decades-stable, and on nothing in prikk.** It can therefore be built, tested, and **stabilized to a durable contract now**, while the **encode-to-prikk half waits on prikk-side decisions still open** (§11 UD, §12 OQ). This document keeps the two halves' obligations separable so the first half can reach stability independently — that separation is a requirement (PU-6), not merely a schedule.

The governing sentence, inherited from RFC 113 and binding on everything below: **facts derive, judgment is authored, and the join is checked.** A migration tool that silently guesses produces history nobody can trust; where approximation is unavoidable, it must be labelled; and every requirement here is written to survive a hostile reader asking *"how would anyone know if this were wrong?"*

---

## 0. The two things a reader must hold before the requirements make sense

**(a) prikk records what these systems do not, and cannot be made more precise than its source.** prikk's atom is an *operation against node identity* — a stable identity for a file across renames and edits. Git has no node identity (renames are inferred at read time); **Mercurial records a rename when the author used `hg mv`/`hg cp` — a stated fact brygge can carry — but not otherwise, and even then it is a path-level copy, not prikk's stable cross-history node identity**; SVN records path copies, not node identity; CVS has no atomic commit at all. So an importer must either **infer** what prikk requires or **admit it did not have it** — and inference is a guess, which prikk's whole proposition forbids doing silently. Mercurial narrows *how often* brygge must infer; it does not remove the boundary. This is not a difficulty to engineer away; it is a boundary to record.

**(b) prikk's receiving surface for imports is *ruled but not built*.** This is the single most important fact for scoping brygge, and it is easy to get wrong. RFC 113 §4.1/§4.2 and §3.1 are *ruled*; the runnable prikk surface behind them is not:

| Thing brygge would need on the prikk side | State in prikk 0.27.1 | Consequence for brygge |
|---|---|---|
| An `Attestation` carrying import provenance | Type defined and gated, **never constructed in production**; current fields (`policy_version`, `plugin_set_hash`, plugin `results`, Pass/Warn/Fail status) are **audit-shaped, not import-shaped** — using it for imports "is a format change if so" (RFC 113 §4.1) | brygge cannot assume an import-attestation shape exists; it is a **prikk-side dependency (UD-1)** |
| An `Import` block kind | Code defined, but **currently refused** by prikk's block validator ("Block kind is not authorized") | brygge cannot produce a prikk-storable import block today — **UD-2** |
| Permission to seal imported history, and by whom | **Open owner question** (RFC 113 §4.4) | brygge must not assume imports can be sealed — **OQ / UD-3** |
| `Unverifiable` AUTHOR-signature outcome | **Exists and is correct** for this (DC-53) | The one piece brygge *can* rely on today |

The honest reading: brygge can be built and be genuinely useful **now** for decode → IR, IR inspection, IR-to-IR comparison, and producing a *proposed* prikk object set for review — but it **cannot land verified, sealable prikk imports** until UD-1…UD-3 land. Requirements below separate "what brygge owes" from "what brygge is waiting on."

---

## 1. Purpose (PU-…)

- **PU-1 — Carry history out of Git, Mercurial, SVN, and CVS (and further sources over time) into an IR that belongs to no system.** Decode is the first half of the value and stands on its own: a faithful IR of a source repository is useful before any encoder exists.
- **PU-2 — Encode prikk from the IR** as the first target, satisfying prikk's import contract (RFC 113) once its receiving surface exists (UD-1…UD-3). Until then, encode a *reviewable proposal*, not sealed history.
- **PU-3 — Be a reusable VCS-abstraction, not a prikk-only feeder — in both directions.** The IR is the reusable core. **Targets:** a second target's encoder must be writable against the IR alone, without reading brygge's prikk encoder and without brygge changing; prikk is the **first and most demanding** client (it needs node identity and provenance), not the only one, and a snapshot-based target must also be encodable from the same IR. **Sources:** a new source (the owner names Git, Mercurial, Subversion, CVS, *"etc."*) must be a **new decoder implemented against the shared IR obligations (§10)**, not a modification of the IR or of existing decoders. Source-extensibility and target-extensibility are symmetric properties of the same IR; neither may bake in the other side's assumptions (IR-1/IR-6).
- **PU-4 — Make the fidelity of an import legible to the person who ran it and to a later third party** — the honesty and verification requirements (§4, §5) are the product, as much as the bytes.
- **PU-5 — Carry the dependency weight prikk refuses to.** brygge exists *because* a Git decoder needs `gix` (~100 crates) or `libgit2` (C), which would be a step change in prikk's five-crate audited surface. brygge owns that weight so prikk stays small; this is a purpose because it governs brygge's boundaries (§8) — brygge's output must be checkable by a prikk that never links a single brygge dependency.
- **PU-6 — The decode → IR half is independently stabilizable, and stability is a deliverable.** Because decode depends only on the (decades-stable) source systems and not on prikk, the decode/IR half must be able to reach a **durable, versioned contract** (IX-07 for the IR; a stable tool surface for `decode`/`inspect`/`verify`) **before** the encode-to-prikk half is finished — indeed before prikk's receiving surface exists at all. Every requirement is written so that decode, the IR, inspection, and internal/against-source verification form a complete product on their own (PU-1), with the encoder as a separable consumer. "Stabilize the first half" is therefore a property the requirements must not obstruct: no decode/IR obligation may be defined in terms of a prikk-side dependency (§11).

## 2. Non-goals (NG-…) — stated as firmly as the goals

- **NG-1 — Not a Git/Mercurial/SVN/CVS compatibility or interoperability layer.** One-way import, never a wrapper, never reading `.git/`/`.hg/` as live storage for a running prikk. (RFC 113 §7.)
- **NG-2 — Not round-tripping.** Exporting prikk (or any target) history *back* to a source system is a separate project and not implied here.
- **NG-3 — Not authorship laundering.** No mechanism may make imported history appear natively authored or natively verified. Imported authorship is `Unverifiable` by construction, and brygge must never present it otherwise. This is the failure RFC 110 §4 names ("manufactured verification"), and it is the one brygge is most able to commit by accident.
- **NG-4 — Not a promise of completeness.** A refusal of an unsupportable source feature is a *feature* (§7's floor), not a bug.
- **NG-5 — Not making a source more precise than it was.** brygge must never emit an IR record or target object that asserts a guarantee the source did not make (e.g., a recorded rename where the source only had delete+create) **unless** the record also states that brygge derived it (HO-1).
- **NG-6 — Not the target's authority.** brygge does not decide whether an import may be trusted, sealed, or admitted. It produces material *about which* the target makes those decisions. (§8.)
- **NG-7 — Not a general backup or archival tool.** The deliverable is history translated into the IR/target, with recorded loss — not a byte-for-byte copy of the source repository.

## 3. What a successful migration must preserve (PR-…)

The test, from RFC 113 §3.1, is not "everything" (unachievable) and not "what looks important" (drifts). It is: **preserve what a future reader's ability to *check something* depends on.** What that reader checks is faithfulness-to-source, so:

- **PR-1 — Content.** The actual file bytes at each recorded state. A migration that loses content is not a migration.
- **PR-2 — Structure and ancestry.** The parent/child relationships between the source's atoms (commits/revisions/reconstructed changesets), as the source expressed them — including multi-parent merges up to whatever the target's floor accepts (§7).
- **PR-3 — Messages and authorship metadata, as *claims* not verified facts.** Author name/email/time, committer, message text — carried as what the source *asserted*, never elevated to what the target *verified*. (This is where NG-3 lives operationally.)
- **PR-4 — The source's own identifiers and signatures, preserved opaquely.** A Git commit SHA and its GPG signature, an SVN revision number, a CVS per-file revision tag: these verify *nothing in the target*, but they are **the only cryptographic/identifier link back to the original**, and they are what lets a third party check the import against the source it claims to come from (VF-2). RFC 113 §3.1 names this as the field most likely to be dropped as "useless here" and most damaging to lose. **brygge must preserve it opaquely, never discard it as unverifiable-in-target.**
- **PR-5 — The parameters that governed any inference.** Which similarity threshold produced a rename, which clustering window produced a CVS changeset, which branch convention was assumed for SVN — recorded so the same output can be reproduced and the judgment reviewed (VF-1, HO-1).
- **PR-6 — The import's own provenance:** what source it was made from (source repository identity, source atom identifiers), the brygge version and decoder version, and the encode target and its version. This is what makes an import reproducible and comparable (VF-1) rather than an unaccountable one-time act.

### What brygge is permitted to lose — but only if the loss is recorded (PR-loss)

Two classes are safe to omit, per RFC 113 §3.1:

- **PR-7 — Representation rather than assertion:** packfile layout, delta encoding, index/working-copy state, reflogs — reconstructible or purely local. May be dropped; the *class* dropped is still stated (HO-2).
- **PR-8 — Advisory data known to be unreliable:** SVN mergeinfo is the standing example. **Preserving wrong data as if authoritative is worse than dropping it** — but the drop is recorded (HO-2), because a reader must not mistake "brygge chose not to carry this" for "the source did not have it."
- **PR-9 — The one class never safe to omit silently: anything whose absence makes a remaining claim look stronger than it is.** If dropping a datum would let a reader over-trust what remains, it must be carried or its absence loudly stated. This is the load-bearing preservation rule; PR-7/PR-8 are its permitted exceptions, not competitors to it.

## 4. How honesty is enforced (HO-…)

Honesty is not a report brygge *offers*; it is a property of every object brygge *writes*, so that it cannot be skipped, lost in a log, or separated from the history it describes.

- **HO-1 — Derived ≠ stated, in the object itself.** Any IR record or target object that brygge *derived* (a rename inferred from similarity, a CVS changeset reconstructed from per-file revisions, an SVN branch inferred from a path copy) must carry, in the record, that it was derived and by what parameters (PR-5). A reader must be able to tell an asserted fact from a brygge judgment **without re-running the heuristic** — because re-running it is exactly what a different brygge version would do differently (RFC 113 §4.2). "Facts derive, judgment is authored, the join is checked": the derivation is marked at the operation, not reconstructed at read time.
- **HO-2 — The boundary of loss is recorded.** Every import states what *class* of information it dropped and what it derived, so a reader knows the *shape* of the loss without re-deriving it (PR-7…PR-9). A known limit written down is a property; the same limit unwritten is a defect waiting for whoever trusts the output.
- **HO-3 — Authorship is `Unverifiable`, surfaced, never dressed up.** Imported history is "present, readable, and not verifiable as authored." brygge's target encoding must land it in exactly the target's vocabulary for that (for prikk, `Unverifiable`), and brygge's own presentation must never show imported authorship as sound, verified, or native (NG-3).
- **HO-4 — The fidelity summary is unskippable and travels with the import.** The person who ran the import must be told, at the end of a run and durably, what was preserved, what was derived (with confidence/parameters), what was dropped, and what was refused — and this summary must be **derivable from the objects themselves** (HO-1/HO-2), not a side report that can drift or be deleted. "Where they are told it" is: at completion, and in the import's own provenance record, such that a later reader recovers the same summary from the imported objects without brygge present.
- **HO-5 — No honesty control may be configurable off.** Verbosity may vary; the *existence* of the derived-vs-stated marking, the loss boundary, the `Unverifiable` status, and the fidelity summary may not be suppressed by any flag. A migration tool whose honesty is optional produces history nobody can trust.

## 5. Verification — what "faithful" can mean when the source guaranteed less than the target records (VF-…)

"Safely preserved" cannot mean "verified," because the target cannot verify what the source never signed. It must mean **reproducible and comparable** (RFC 113 §3).

- **VF-1 — Determinism.** Re-running the same brygge version over the same source with the same parameters produces the **same IR** and the **same target objects** — or the difference is explainable by a stated cause. Determinism is what lets a third party check the claim at all; non-determinism makes "faithful" uncheckable. (Note the tension with content-addressed targets: see UD-4.)
- **VF-2 — Round-checkable against the source.** Because the source's own identifiers and signatures are preserved opaquely (PR-4), a third party holding the *original source repository* can check that brygge's output corresponds to it — commit-by-commit / revision-by-revision — without trusting brygge. This is the strongest form of "faithful" available, and it is the reason PR-4 is non-negotiable.
- **VF-3 — Internally checkable without the source.** A reader holding only the import can check: that every derived record is marked (HO-1); that the loss boundary is stated (HO-2); that authorship is `Unverifiable` (HO-3); that content and ancestry are internally consistent; and that the provenance record names its source and parameters (PR-6). This does not prove faithfulness-to-source (only VF-2 can), but it proves brygge did not *hide* anything, which is a distinct and checkable property.
- **VF-4 — The two claims are never conflated.** "prikk verified this" and "this was faithfully imported and prikk verified nothing about its authorship" must be distinguishable by any reader at any time (RFC 113 §3, third point). brygge must make the second claim in a form the target already distinguishes from the first — for prikk, that is `Unverifiable` plus provenance-in-an-attestation, *not* history that looks native.
- **VF-5 — Faithfulness is defined per source, and the definition is published.** Because Git, Mercurial, SVN, and CVS guaranteed different things, "faithful" means something different for each (§7). brygge must state, per source, what faithfulness *can* mean for it and what it explicitly cannot — so a user's expectation is set before they run, not corrected after.

## 6. Idempotence and re-runs (ID-…)

- **ID-1 — Re-importing the same source is defined, not accidental.** Running brygge twice over the same source must either produce an equivalent result (VF-1) or state precisely why it differs (new brygge version, changed parameters, changed source). "I ran it again and got different history" must never be a silent outcome.
- **ID-2 — Re-import is not a merge.** brygge does not attempt to reconcile a re-import with a previously-imported-and-then-locally-modified target; that is the target's history-manipulation surface, not brygge's (BN-4). brygge's obligation is a *fresh, deterministic* translation whose relationship to a prior import is stated (same source + same params + same version → same objects), leaving reconciliation to the target.
- **ID-3 — Provenance makes double-import detectable by the target.** Because each import records what it was made from (PR-6), a target *can* recognize that two imports share a source; whether it deduplicates, refuses, or admits both is the **target's** decision (BN-3), not brygge's — but brygge must supply enough provenance for the target to make it.
- **ID-4 — Content-addressed determinism caveat.** For a content-addressed target (prikk), identical input objects yield identical ids *only if every identity-bearing field is itself deterministic*. brygge must therefore make every field it controls deterministic (VF-1) and must **name every field it cannot** (e.g., an import timestamp, if the target's provenance object carries authoritative time) as a stated non-determinism, not let it silently perturb ids. See UD-4.

## 7. The sources are not one problem (SRC-…)

brygge must treat each source separately and publish, per source, what faithfulness can mean (VF-5) and what is refused rather than approximated (the floor — an owner decision, OQ-3/§11). Git, Mercurial, SVN, and CVS differ enormously in what they *guaranteed*, and the IR (§10) exists precisely so those differences are recorded, not flattened. A future source ("etc.", PU-3) is added as a new decoder that answers these same questions in its own terms.

The four named sources form a **difficulty gradient**, which is also the recommended build order: Git (a real DAG, identity inferred) → **Mercurial** (a real DAG, identity *often stated*) → SVN (revisions atomic, branches by convention) → CVS (no atomic commit at all). Each earlier source de-risks the IR before the next stresses it.

### SRC-Git — hard but tractable
- **SRC-G1** Content-addressed, atomic commits, a real DAG: content, ancestry, and messages import faithfully as *claims*.
- **SRC-G2** The two hard problems are **identity inference** (renames: Git records delete+create, prikk wants a node identity — every inferred rename is marked derived with its similarity parameters, HO-1) and the **feature floor** (submodules, octopus merges beyond the target's parent limit, replace refs, grafts, shallow clones — refused, not approximated; the exact list is the owner's, OQ-3).
- **SRC-G3** GPG-signed commits: the signature is preserved opaquely (PR-4) and verifies *nothing in the target*; brygge must never present a GPG-signed Git commit as a verified prikk author (NG-3).

### SRC-Hg (Mercurial) — tractable, and epistemically *closer* to prikk than Git
- **SRC-H1** Like Git, Mercurial has content-addressed, atomic changesets and a real DAG, so content, ancestry, and messages import faithfully as *claims* (PR-1/2/3) with the same discipline as SRC-Git.
- **SRC-H2 — Renames are frequently *stated*, not inferred, and this must be honoured.** Mercurial records copy/rename metadata in filelogs (via `hg mv`/`hg cp`, or commit-time similarity the *source* chose to record). Where the source **recorded** a rename, it is a **source-stated fact** and must be carried as *stated* (not re-derived, not marked derived) — a genuine advantage over Git that reduces the derived-marking burden (HO-1). Where the source did **not** record it (a plain remove+add), brygge carries delete+create as stated, and only marks a rename *derived* if it chooses to infer one — with its parameters (PR-5). The IR must be able to hold both "rename, stated by source" and "rename, derived by brygge" as distinct epistemic states (IR-2); collapsing them would throw away exactly the fidelity Mercurial uniquely offers.
- **SRC-H3 — Mercurial-specific structure needs floor rulings (OQ-3).** hg carries concepts with no clean prikk analogue: **named branches** (a branch name embedded in the changeset) versus **bookmarks** (movable Git-like refs) — two branch models that may coexist; **phases** (`public`/`draft`/`secret` — largely local workflow state, representation not assertion, PR-7); **obsolescence markers / evolve** (metadata about rewritten changesets — advisory, PR-8); **multiple heads per named branch**; and **`.hgtags`** (tags stored *as versioned history*, a quirk that is itself content to preserve). brygge must map named branches and bookmarks to the IR's branch-identity notion **without silently privileging one**, drop-with-record the representation-only parts (phases, obsmarkers) per HO-2, and refuse-or-flag anything the owner's floor names (OQ-3).
- **SRC-H4** Mercurial's node ids and any commit signatures are preserved opaquely (PR-4) and verify *nothing in the target* (NG-3).

### SRC-SVN — different, not easier
- **SRC-S1** Atomic revisions help ancestry, but **branches and tags are path copies, not first-class refs** — branch identity must be reconstructed by convention (`/trunk`, `/branches/x`, `/tags/x`) that many real repositories violate. Every reconstructed branch is a **derived** record (HO-1); a repository that violates the convention must be **refused or flagged**, never silently mis-branched.
- **SRC-S2** **Mergeinfo is advisory and frequently wrong** (PR-8): brygge must not import it as authoritative ancestry; it is dropped-with-record or carried-as-advisory-and-labelled, never promoted to a real merge parent.
- **SRC-S3** SVN's per-path copy semantics do not map to node identity any better than Git's; the same derived-marking discipline applies.

### SRC-CVS — a research problem, and the honest deliverable may be lossy
- **SRC-C1** **There are no atomic commits.** A changeset must be *reconstructed* from per-file revisions by clustering on author, message, and time window — the `cvs2svn`-lineage approach, imperfect by nature.
- **SRC-C2** **A CVS import cannot be more faithful than its reconstruction.** Every reconstructed changeset is a **derived** record carrying its clustering parameters (HO-1), and the fidelity summary (HO-4) must make the reconstruction's uncertainty prominent, not buried.
- **SRC-C3 — The honest position, stated plainly (the task's hardest ask):** a *faithful* CVS import in the sense VF-2 offers for Git (round-checkable against a canonical source atom) is **not achievable**, because CVS has no atomic source atom to check against — the changeset is brygge's construction, not CVS's record. The honest deliverable is therefore a **lossy, explicitly-labelled reconstruction**: content and per-file history preserved faithfully; changeset grouping preserved *as brygge's derived judgment*, reproducible (VF-1) and reviewable (VF-3) but never claimed as the source's own fact. brygge must tell a CVS user this *before* they run (VF-5), so "faithful CVS import" is never a promise brygge made and broke.

## 8. Boundaries — brygge's responsibility vs. the target's (BN-…)

- **BN-1 — brygge owns: decode, the IR, encode, and honesty about all three.** It is responsible for reading the source correctly, representing it faithfully-with-provenance in the IR, translating the IR into target objects, and making every derivation and loss legible (§4).
- **BN-2 — The target owns: admission, trust, verification, and storage.** Whether the produced objects are accepted, whether they are trusted, whether they are sealed, and how they are stored are the target's decisions. brygge produces material *about which* the target decides (NG-6).
- **BN-3 — The provenance interface is the boundary.** brygge's obligation to the target is to supply provenance (PR-6) sufficient for the target to make its admission/trust/dedup/seal decisions; the target's obligation is to have a place to put it and a policy about it. For prikk that place is an `Attestation` (RFC 113 §4.1) — which does not yet fit (UD-1).
- **BN-4 — brygge never manipulates target history.** No merge, rebase, reconcile, or seal on the target's behalf. A re-import is a fresh translation (ID-2); what the target does with it is the target's.
- **BN-5 — brygge's dependency weight stops at its own boundary.** brygge may depend on `gix`/`libgit2`/SVN/CVS libraries freely (PU-5), but **nothing brygge produces may require the target to link any of them**. A prikk repository must be able to verify a brygge import using only prikk's own five-crate surface; the import's checkability (VF-3) must not route through a brygge dependency. This is the concrete meaning of "brygge carries the weight, prikk does not."

## 9. Failure behaviour (FA-…)

- **FA-1 — Partial imports are recorded, never silently truncated.** If an import stops (interrupted run, source error, an unsupportable feature hit mid-stream), what was and was not imported must be stated, and the partial result must be distinguishable from a complete one — a half-imported history that looks whole is a manufactured-verification failure (NG-3).
- **FA-2 — A source that violates its own conventions is refused or flagged, never guessed.** The SVN-branch-convention violation (SRC-S1) is the archetype: brygge does not silently pick an interpretation. It refuses, or it imports with the violation loudly recorded as a derived judgment (HO-1) the user must accept.
- **FA-3 — An unsupportable source feature is refused with a named reason, not approximated.** The floor (§7, OQ-3) produces refusals; each refusal names the feature and why it is refused, so the user knows exactly who can migrate and who is told no.
- **FA-4 — Interrupted runs leave no ambiguous target state.** brygge must not leave the target holding objects that appear complete but are not; because sealing/admission is the target's act (BN-2), brygge's safe failure mode is to produce a clearly-incomplete-and-labelled proposal that the target has not admitted, rather than a partially-admitted history.
- **FA-5 — Determinism survives failure.** Re-running after a failure reproduces the same result up to the failure point (VF-1); a failure is not a source of divergence.

## 10. The IR's obligations (IR-…) — requirements on it, not a schema for it

The IR is the reusable core (PU-3) and the place all three honesty disciplines live. Its obligations, as requirements:

- **IR-1 — The atom asserts what its source could guarantee, not what a target wants.** Designed for **faithfulness-with-provenance, not neutrality**: a neutral IR converges on "snapshots plus metadata" (the common denominator), which is exactly what prikk is *not*, and offers nowhere to record that identity inference happened (RFC 113 §3.1). The IR must be able to hold *less* than prikk wants (a snapshot with no node identity) **and** to carry the marker that a node-identity encoder will have to infer the rest — so that inference happens in the encoder, visibly, not hidden in the IR.
- **IR-2 — Every record carries its epistemic status.** Stated-by-source vs. derived-by-decoder is a property of every atom (HO-1). A CVS reconstructed changeset and a Git commit may occupy the same IR slot while being epistemically different things; an IR that cannot say which is **lying by omission** and is non-conformant.
- **IR-3 — The source's opaque identifiers and signatures are first-class IR content** (PR-4), not decoder-local scratch discarded before encode. The IR is where the cryptographic link back to the source is preserved for VF-2.
- **IR-4 — The loss boundary is representable in the IR** (HO-2): the IR can state what a decoder dropped and what it derived, per import, so any encoder reproduces the same fidelity summary (HO-4) and two imports are comparable.
- **IR-5 — The IR is encoder-agnostic and decoder-shared.** An encoder for a second target must be writable against the IR's stated obligations alone (PU-3), and the answers to "what is a record / what is preserved / what is omitted" must be **shared across decoders** even though they resolve differently per source — otherwise the IR cannot compare a Git import to a CVS one and "how faithful was this?" stops having an answer (RFC 113 §3.1 closing rule).
- **IR-6 — The IR privileges no target's identity model, but can carry any target's needs.** It must not bake in prikk's `NodeId` (that would make it useless to a snapshot target), yet it must be *expressive enough* to let a node-identity encoder record inferred identity with provenance. The IR's job is to be the honest substrate; identity inference is the encoder's authored judgment, marked as such.

## 11. prikk-side dependencies brygge is waiting on (UD-…)

These are prikk's to build (RFC 113 §4); brygge's requirements name them so no requirement above silently assumes them. They gate PU-2 (real prikk imports), not PU-1/PU-3 (decode, IR, other targets).

| ID | Dependency | prikk state today | What it gates |
|---|---|---|---|
| **UD-1** | An `Attestation` shape that fits import provenance | Type defined, never constructed; current fields are audit-shaped; import use "is a format change" (RFC 113 §4.1, RFC 114 frozen surface) | BN-3, VF-4 for prikk — brygge cannot emit a prikk import attestation until its shape exists |
| **UD-2** | An authorized `Import` block kind (or a ruling that imports use `Normal` blocks) | `Import` kind **defined but refused** by the block validator | Whether brygge can produce prikk-storable import blocks at all |
| **UD-3** | A ruling on whether imported history may be sealed, and by whom | Open owner question (RFC 113 §4.4) | Whether a brygge import can become sealed prikk history or only an unsealed proposal |
| **UD-4** | A deterministic identity contract for import-time fields | prikk pins history `created_at=0`, but `AttestationPayload` carries an **authoritative** `created_at` | ID-4 / VF-1 — brygge must know which import fields are identity-bearing so re-runs are reproducible |
| **UD-5** | Format stability (Badge criterion 2) and sync (criterion 1) | Both open (RFC 113 §6) | Writing "the largest repositories in the project's life against an unstated format contract" is a risk brygge must not take before the format is stable; and an import that cannot be exchanged is half a migration |

Until UD-1…UD-3 land, brygge's prikk encoder produces a **reviewable proposal** (labelled, unsealed, `Unverifiable`), never sealed history — and says so.

## 12. Open questions — the ones that are not brygge's to answer (OQ-…)

Per RFC 113 §4.3–§4.5, these belong to the **receiving project's owner**. brygge names each, states what it changes downstream, and stops.

- **OQ-1 — What, if anything, the importer signs.** *"I imported this from that source"* is a true, signable claim; *"this person authored this"* is not the importer's to assert. **Downstream effect:** whether every prikk import carries a signed import-attestation (and what `verify` says about an import with none) — it changes what a bundle receiver sees and whether an import has any authenticated provenance at all. This is DC-35 territory (who may sign what). **Not brygge's.**
- **OQ-2 — Whether imported history may be sealed at all, and by whom.** Sealing is a maintainer act with a verified signature; a maintainer sealing imported blocks makes a real inclusion claim. **Downstream effect:** whether a migration ends in native sealed history or a permanently-unsealed imported tier — the difference between "migrated to prikk" and "readable in prikk." **Not brygge's** (UD-3).
- **OQ-3 — The source-side floor, per source.** Which features are **refused** rather than approximated (Git submodules/octopus/grafts/shallow; SVN convention-violating layouts; CVS reconstructions below a confidence bar). **Downstream effect:** this decides *who can migrate and who is told no* — product scope, not engineering. brygge implements whatever floor is set and refuses cleanly below it (FA-3); it does not set the line.
- **OQ-4 — (surfaced, downstream of OQ-1) What `verify` reports for an import.** If OQ-1 yields a required import-attestation, `verify` must have something to say about its presence/absence; if it yields none, imported blocks are `Unverifiable`-authored with no provenance object. This is the receiving project's to specify; brygge must be able to target either answer.

---

## Traceability (task coverage)

| Task requirement | Where |
|---|---|
| Purpose and non-goals, non-goals as firm as goals | §1 PU, §2 NG |
| What must be preserved / permitted to lose, loss recorded | §3 PR (PR-9 the load-bearing rule) |
| How honesty is enforced; where told; impossible to skip | §4 HO (HO-4 unskippable, HO-5 non-configurable) |
| Verification; what "faithful" means when source guaranteed less | §5 VF (VF-2 round-check, VF-5 per-source) |
| Idempotence and re-runs | §6 ID (ID-4 the content-addressed caveat) |
| Failure behaviour | §9 FA |
| Boundaries: brygge vs. target | §8 BN (BN-5 the dependency-weight boundary) |
| The IR's obligations as requirements | §10 IR |
| Git / Mercurial / SVN / CVS treated separately; hg stated-renames; CVS honesty | §7 SRC (SRC-H2 stated vs derived renames; SRC-C3 the lossy-but-labelled verdict) |
| Source-extensibility (a new source = a new decoder) and the decode/IR half's independent stability | PU-3 (sources), PU-6, §0 intro |
| Given decisions honoured (attestation-not-payload; derived-marked; boundary-recorded; Unverifiable) | HO-1, HO-2, HO-3, BN-3, and threaded throughout |
| Questions not to answer, with downstream effect, then stop | §12 OQ |
| Uncertainty stated as uncertainty | §0(b), §11 UD, SRC-C3, ID-4 |

*End of brygge Requirements v0.2. This document is the contract a design must satisfy; it deliberately contains no architecture, schema, or API. What is buildable now vs. gated, precisely: the **decode → IR half** (the IR obligations §10, and the Git/Mercurial/SVN/CVS **decoders**, inspection, and internal/against-source verification) depends only on the source systems and can be designed and **stabilized now** (PU-6). The **encode-to-prikk** path past a reviewable proposal waits on the prikk-side dependencies §11 (UD-1…UD-3) and the owner's open questions §12 (OQ-1…OQ-3), exactly as RFC 113 §4a states. Build order follows the §7 difficulty gradient: foundations → Git → Mercurial → SVN → CVS.*
