# RFC 007 D-9 — additive-fit confirmation against the shipped `brygge-ir`

**What this is.** RFC 007 D-9 requires confirming, before the `brygge-decode-cvs` build spec assumes
anything, that CVS fits the **frozen IR contract 1.0.0** (RFC 003 D-7) — with at most *additive* changes,
never a breaking one. CVS is the gradient's hardest source and the first to make the **`ChangeAtom` itself
`Derived`**, so this is the deepest test the freeze has faced.

**Method.** Checked each CVS construct, and D-9's candidate need, against the shipped types in
`crates/brygge-ir/src/{model,status,honesty,version}.rs` at brygge `66b52d8`. `brygge-ir` is **unchanged
since the freeze** (`fe52c63`); citations are to those files.

## Headline

> **CVS fits IR 1.0.0 with ZERO contract changes.** Every construct maps onto a type that already exists in
> the frozen contract — and the two that matter most were written *naming CVS*: the `Derived` atom status
> and `DerivationKind::ReconstructedChangeset` (whose doc reads "A changeset reconstructed from per-file
> revisions (CVS)"). The freeze holds for the source designed to break it: even a decoder whose atoms are
> all judgments needs nothing added.

This is the same outcome as SVN, one level deeper — SVN made *refs* `Derived` over a `Stated` atom spine;
CVS makes the **atom** `Derived`, and the contract already had the slot.

## The candidate needs, resolved

### The `Derived` changeset atom (SRC-C1, IR-2). **Already expressible. No change.**

`ChangeAtom.status` is an `EpistemicStatus` (`model.rs:159`), so an atom can be
`Derived(Derivation { kind: DerivationKind::ReconstructedChangeset, by: "brygge-decode-cvs", params:
{"window": …, "cluster_keys": "author,log"}, confidence: Some(_) })`. The taxonomy variant is
purpose-built (`status.rs:44-45`):

> `/// A changeset reconstructed from per-file revisions (CVS). Params must include the clustering keys.`
> `ReconstructedChangeset,`

`honesty::summary` walks `atom.status` (`honesty.rs:48`), so **every** CVS atom counts on the fidelity
report as `derived.reconstructed-changeset=<all atoms>` — which is exactly SRC-C2's "uncertainty prominent,
not buried," delivered as a checkable property rather than a promise. Nothing to add.

### Reconstruction confidence (SRC-C2, D-3/D-8). **Already expressible. No change.**

`Derivation.confidence: Option<u8>` (`status.rs:36`, "an optional confidence in `0..=100`") carries the
per-changeset confidence that the floor (OQ-B) tests. Purpose-built; no new field.

### Per-file revision preservation (PR-4). **Expressible within 1.0.0; one within-contract encoding choice (OQ-D).**

A reconstructed changeset has no single source-native id — it groups *N* per-file RCS revisions. The opaque
`SourceIdentity.atom_id: Vec<u8>` (`model.rs`) holds the **canonical set of `(path@rev)` pairs** the
changeset groups; the field doc already anticipates this ("Opaque atom identity (e.g. a Git commit SHA, an
SVN revision, **a CVS revision tag**)"). This keeps the source-native per-file revisions — which *are* real
and checkable — recoverable from the atom, so the per-file VF-2 correspondence (D-7) has something to check
against, even though the *changeset* is brygge's construction.

- **OQ-D resolution:** pack the sorted `(path@rev)` set into `atom_id` for M4. A *structured* per-op
  source-revision field (one revision per `PathOp`) is the deferred **additive** minor bump — proposed only
  if a consumer needs per-op source ids — never a break. This mirrors the SVN D-9 decision (reuse an
  existing construct; defer the structured addition until a consumer appears).
- **One honesty note for the handoff:** for CVS, `atom_id` is the first case where the atom identifier is
  **brygge-constructed** rather than source-native. That is consistent, not dishonest: the atom is already
  marked `Derived`, so its identifier being brygge's construction is expected — and the *contents* of that
  identifier (the `(path@rev)` pairs) are source-native and verifiable. The handoff must encode them
  canonically (sorted, stable) so re-decode is byte-identical (VF-1).

### Tags and branches (D-5). **Already expressible. No change.**

Reconstructed CVS tags and branches are `Derived` `RefRecord`s reusing `DerivationKind::ReconstructedBranch`
(`status.rs:46`) — the same mechanism-name decision as SVN's D-9, with `RefKind::Tag`/`Branch` carrying the
distinction and a straddle/per-file caveat in `params`. `SourceKind::Cvs` (`model.rs:32`) already exists.

## Full CVS → IR 1.0.0 coverage

| CVS construct | IR 1.0.0 representation | Status |
|---|---|---|
| Reconstructed changeset | `ChangeAtom { status: Derived(ReconstructedChangeset, params, confidence) }` | Derived |
| Its clustering parameters | `Derivation.params` ("window", "cluster_keys") | — |
| Its confidence | `Derivation.confidence: Option<u8>` | — |
| Its per-file `(path@rev)` set | canonical bytes in opaque `SourceIdentity.atom_id` (OQ-D) | opaque, source-native |
| Repository UUID / root | `SourceIdentity.repo_id` (the CVSROOT path or a stable root marker) | opaque |
| Per-file add / edit / `dead` delete | `PathOp::{Add,Modify,Delete}` (`model.rs`), content → store | Stated (faithful) |
| `-kb` / exec mode | `PathOp` `mode` (regular / executable) | Stated |
| author / date / log | `MetadataClaims` (date = a recorded representative) | claim |
| Tag (per-file symbolic name) | `RefRecord { kind: Tag, status: Derived(ReconstructedBranch, params: straddle caveat) }` | Derived |
| Branch (magic branch number) | `RefRecord { kind: Branch, status: Derived(ReconstructedBranch) }` | Derived |
| Keyword expansion / `-kb` translation | stored bytes carried; `DropRecord` (Representation) | dropped-with-record |
| `CVSROOT` admin files, locks, working copy | `DropRecord` (Representation) | dropped-with-record |
| — no merge tracking in CVS — | (nothing to carry; a merge is just more per-file revisions) | N/A |
| — no rename in CVS — | delete+add, **no `RenameHint`** by default (OQ-C) | Stated (faithful) |

**The one thing the IR does *not* need a field for, by design:** the *absence* of changeset-level VF-2
(D-7). That is a `verify`-surface behaviour (per-file correspondence + VF-1 determinism, not changeset
correspondence) and a published-faithfulness-statement matter (VF-5) — not an IR construct. The per-file
content lives in the ops and the blob store; nothing in the contract needs to change to make the honest
"no changeset-level round-check" true.

## Secondary observation

None new at the contract level. The SVN D-9 surfaced the `version.rs` enum-forward-compat nuance and the
`C-4b` subprocess-isolation refinement, both since folded into `brygge-03` v0.2; CVS Tier R adds a new
untrusted-input parser (RCS) but **no subprocess and no dependency**, so it raises no new threat-model
delta here — the RCS parser's bounds/panic-safety are the subject of the forthcoming security review, not
of this additive-fit check.

## Conclusion

- **CVS fits IR 1.0.0 with no contract change.** The `brygge-decode-cvs` build spec may assume the frozen
  types as-is; it introduces **no** new IR field or variant.
- One within-contract encoding choice carries into the handoff: the reconstructed changeset's `atom_id`
  packs the canonical `(path@rev)` set (OQ-D); a structured per-op source-revision field is the deferred
  additive if a consumer ever needs it.
- The freeze is now demonstrated against **all four** sources on the gradient — Git and hg (pre-freeze,
  the freeze basis), SVN and CVS (post-freeze, additive-only) — the last of which makes the atom itself a
  judgment and *still* needs nothing added. That is the strongest evidence the contract could have for
  IR-5/IR-6 and RFC 003 D-7.
