# RFC 002 — Honesty &amp; provenance machinery

**Status.** Proposed (2026-09-03). Builds on RFC 001 (Accepted): RFC 001 made honesty a property of the
IR *model* (`Stated`/`Derived` on every assertion, renames as literal ops + a marked hint, a
`LossBoundary` and a pure `summary`). This RFC defines the **machinery and contracts** that turn those
model properties into guaranteed, non-suppressible, comparable, and recoverable *behaviour*: the
derivation and loss taxonomies, the fidelity-report contract, provenance completeness, and the
non-suppressibility rule.
**Tracks.** ROADMAP Phase A0; requirements §4 (`HO-1…HO-5`), §3 (`PR-3…PR-9`), §5 (`VF-3/VF-4`), §10
(`IR-4/IR-5`); external design §2.3 (`FS-01…06`), §2.4 (`PX-01…03`), `CT-04`, `CL-07`; prikk RFC 113
§3.1. Track A — not prikk-gated (the prikk-specific provenance *object* is RFC 008/UD-1; this RFC fixes
provenance *content*).
**Touches.** `brygge-ir`'s `honesty` module (taxonomies + the fidelity report) and the report format
`brygge summary`/`inspect` emit; no source or target code.

## Summary

RFC 001 guarantees a decoder *cannot* express an inference except as a `Derived` record. This RFC makes
the rest of the honesty promise real: that derivations and losses are drawn from **shared taxonomies**
so a Git import and a CVS import are comparable (`IR-5/IX-06`); that the **fidelity summary is a stable,
versioned, machine-readable contract** recoverable from the objects (`HO-4/FS-02/CT-04`); that
**provenance carries everything a target and a third party need** (`PR-6/VF-2/VF-4/PX-*`); and that
**honesty cannot be configured off** (`HO-5/CF-02`). It is the layer that makes "how faithful was this
import?" a question with a comparable, checkable answer.

## The constraints that scope this design

- **Comparability requires shared vocabularies** (RFC 113 §3.1 closing rule, `IR-5/IX-06`): if each
  decoder invents its own notion of "derived" or "dropped", the IR cannot compare imports and the
  question loses meaning.
- **The summary must be recoverable from the objects, not a side report** (`HO-4/FS-02`): a later reader
  reconstructs it with no run log.
- **Two claims never conflated** (`VF-4`): "verified by the target" vs "faithfully imported, authorship
  unverified" must be distinguishable in every honesty output.
- **The never-silently-omit class** (`PR-9`): anything whose absence makes a remaining claim look
  stronger than it is must be carried or loudly stated.
- **Provenance is separable from history** (`PX-02`, RFC 113 §4.1) and must suffice for the target's
  admission/dedup/seal decisions and for a third party's round-check (`VF-2`).

## Decisions

- **D-1 — A shared *derivation taxonomy*.** `Derived` records carry a `kind` from a closed, versioned
  enum shared across decoders: `InferredRename`, `ReconstructedChangeset`, `ReconstructedBranch`,
  `InferredMerge`, `NormalizedMetadata`, `Other{note}`. Each kind fixes what `params` it must carry
  (e.g. `InferredRename` → similarity threshold + algorithm id; `ReconstructedChangeset` → clustering
  window + keys). This is what makes derivations comparable and the summary cross-source (`IR-5/IX-06`).
- **D-2 — A shared *loss taxonomy*, with the load-bearing class explicit.** `LossBoundary.dropped`
  entries carry a `class`: `Representation` (packfiles, deltas, index, reflogs — reconstructible/local,
  `PR-7`), `AdvisoryUnreliable` (SVN mergeinfo, hg obsmarkers — `PR-8`), or `Other`. Dropping is
  permitted only for `Representation`/`AdvisoryUnreliable`; the **`PR-9` class — absence that would
  strengthen a remaining claim — may never be a silent drop**: it is carried, or recorded as an explicit
  `RetainedAsAdvisory`/`RefusedToDrop` note. A decoder that tries to drop outside the permitted classes
  is a conformance failure.
- **D-3 — The fidelity report is a stable, versioned, machine-readable contract** (`CT-04/CL-07`),
  distinct from the IR. `summary(&Ir) -> FidelityReport` is pure (`FS-02`); the report has a
  human form (default) and a **versioned machine form** (its own `report_version`, so a CI gate can pin
  it) with sections: **preserved**, **derived** (grouped by taxonomy kind, with params/confidence),
  **dropped** (by class), **refused** (floor hits, when a decoder reaches them). The report is
  reproducible byte-for-byte from the artifact — the external proof of `HO-4`.
- **D-4 — Provenance completeness is specified here** (`PR-6/PX-01`). `ImportProvenance` must carry:
  source repo identity + per-atom source ids (via `SourceIdentity`, `PR-4`); the brygge version, decoder
  id + version; **every inference parameter** used (`PR-5`); the target + version when encoding; and a
  reference to the `LossBoundary`. It is a *statement about* the history, serializable and verifiable
  independently of it (`PX-02`) — so a target can carry it as its own object and a third party can run
  `VF-2` from it. The prikk-specific *form* (an `Attestation`) is RFC 008/UD-1; the *content* is fixed
  here.
- **D-5 — The two-claims distinction is structural** (`VF-4/HO-3`). Every honesty output tags authorship
  as `Unverified` (source-claimed) and shows any preserved source signature as *verifying nothing in the
  target*. There is no code path that can render imported authorship as target-verified; the report and
  `inspect` share one renderer for this so they cannot drift.
- **D-6 — Non-suppressibility is enforced by the API, not by discipline** (`HO-5/CF-02`). The
  `brygge-ir` types offer **no** constructor or serializer that omits the epistemic status, the loss
  boundary, or the provenance; there is no "quiet" flag that drops them. Verbosity affects *rendering
  detail* only. A test asserts that a well-formed artifact always yields a complete `FidelityReport`.

## Open questions

- **OQ-A — How much of the merge/branch derivation taxonomy to fix now vs at the source RFCs.** SVN
  branch reconstruction (RFC 006) and CVS changeset reconstruction (RFC 007) will stress the taxonomy;
  this RFC fixes the *frame* and the Git/hg-relevant kinds, and lets later source RFCs *add* kinds
  additively (the enum is versioned, D-1). *Leaning:* fix the frame + `InferredRename`,
  `ReconstructedChangeset`, `ReconstructedBranch` now; add more additively.
- **OQ-B — The machine report format's encoding** — align with RFC 003's codec decision (likely the same
  canonical encoding, but the report is a *tool* contract `CT-04`, versioned separately from the IR).
- *Not owner-gated.* The prikk attestation *object* is gated (RFC 008/UD-1/OQ-1); provenance *content*
  and the fidelity report are not.

## Consequences

- "How faithful was this import?" gets a comparable, machine-checkable answer across sources — the
  property that makes the IR a reusable abstraction rather than three private formats.
- Honesty becomes un-turn-off-able at the API boundary (D-6), closing the "a flag hid the derivation"
  failure mode before any decoder exists.
- The encoder (RFC 008) has a fixed provenance *content* contract to target, so the only thing gated on
  prikk is the *object shape*, not what provenance must say.
- The fidelity report is the CI-gateable artifact a migration pipeline pins (`CL-07/CT-04`).
