# RFC 006 D-9 — additive-fit confirmation against the shipped `brygge-ir`

**What this is.** RFC 006 D-9 requires confirming, before the `brygge-decode-svn` build spec assumes
anything, that Subversion fits the **frozen IR contract 1.0.0** (RFC 003 D-7) — with at most *additive*
changes, and never a breaking one. This is the freeze's first post-freeze test under discipline.

**Method.** Checked each SVN construct, and specifically RFC 006 D-9's three candidate needs, against the
shipped types in `crates/brygge-ir/src/{model,status,honesty,version}.rs` at brygge `40181f3`. Citations
are to those files.

## Headline

> **SVN fits IR 1.0.0 with ZERO contract changes — not even an additive minor bump.** Every SVN construct
> maps onto a construct that already exists in the frozen contract. The freeze holds, and it holds in the
> strongest possible form: the third source needed nothing added, just as Git (M1) and Mercurial (M2)
> needed nothing changed.

This is a stronger result than D-9's own "preliminary finding: no *breaking* change is required, candidate
additive needs to confirm." The candidates turn out to be **already expressible**, and one of them
(`DerivationKind::ReconstructedBranch`) was evidently designed with SVN in mind — its doc names SVN.

## The three candidate needs, resolved

### Candidate 1 — record that a `Derived` branch/tag came from path convention, with the convention as its parameter (PR-5). **Already expressible. No change.**

`EpistemicStatus::Derived` carries a full `Derivation` record (`status.rs:20`, `:26`) with a
`params: BTreeMap<String,String>` and a closed `DerivationKind` taxonomy that **already contains the
purpose-built variant** (`status.rs:46`):

> `/// A branch reconstructed by convention (SVN path copies). Params must include the convention.`
> `ReconstructedBranch,`

So a reconstructed branch is `RefRecord { kind: RefKind::Branch, status: Derived(Derivation { kind:
ReconstructedBranch, params: {"convention": "standard-trunk-branches-tags", …}, by:
"brygge-decode-svn", decoder_version, confidence }), … }` (`model.rs:178` `RefRecord`). The convention is
carried per-ref in `Derivation.params`, exactly as the taxonomy doc mandates. Nothing to add.

### Candidate 3 — distinguish a convention-derived branch from a first-class one. **Already expressible, and a new `RefKind` would be a design regression. No change.**

The distinction is carried by the **status** field, orthogonally to `kind`:

- first-class branch (git/hg): `RefKind::Branch` + `status: Stated`;
- convention-derived branch (svn): `RefKind::Branch` + `status: Derived(ReconstructedBranch)`.

Adding a `RefKind::ConventionBranch` would fold epistemic status into kind — precisely the conflation the
model separates (RFC 006 D-4; `model.rs` keeps `RefKind` and `EpistemicStatus` independent). The existing
encoding is not just sufficient, it is *more correct* than the candidate. Do not add a variant.

### Candidate 2 — record the "tag not guaranteed immutable" caveat on a derived tag. **Expressible within 1.0.0; one decoder-level naming choice to settle (not a contract change).**

A reconstructed tag is `RefRecord { kind: RefKind::Tag, status: Derived(Derivation{…}), … }`. The
immutability caveat lives in that `Derivation.params` — e.g. `params["immutability"] =
"not-guaranteed: an SVN tag is an ordinary directory and may carry post-creation commits"` — recoverable
*from the object*, satisfying honesty-in-the-object (HO-4) rather than deferring the caveat to prose.
Representation is fine: a `RefRecord.target` is a single `AtomId`, so a tag whose directory was committed
to after creation points at the one revision the decoder chose (creation or latest — a handoff decision),
and the params caveat is exactly the warning that a moving thing was pinned to one point.

**The one open choice — decoder-level, within contract — is which `DerivationKind` a reconstructed *tag*
carries**, since the taxonomy has `ReconstructedBranch` but no `ReconstructedTag`:

- **(a) reuse `ReconstructedBranch`** — the *derivation mechanism* (convention over path copies) is
  identical for tags and branches, and `RefKind::Tag` already carries the tag-ness. Keeps the closed
  taxonomy comparable (`derived.reconstructed-branch=N` on the fidelity report). Slightly branch-named.
- **(b) `DerivationKind::Other("reconstructed-tag")`** — precise label, but pollutes the `Other` escape
  hatch and loses the comparability the closed taxonomy exists for.
- **(c) add `DerivationKind::ReconstructedTag`** — a genuinely *additive* minor bump (the taxonomy doc says
  "New kinds are added additively", `status.rs:39`). **Deferred**, per the same discipline as OQ-D: do not
  grow the contract until a consumer needs the branch/tag distinction *at the derivation-kind level* (the
  `RefKind` already distinguishes them for every consumer that reads refs). If that consumer appears, (c)
  is the clean proposal at that time — subject to the forward-compat note below.

**Recommendation for M3:** option **(a)** — reuse `ReconstructedBranch`, carry the tag-immutability caveat
in `params`, and record in the handoff that a first-class `ReconstructedTag` is the deferred additive if a
consumer ever needs it. This ships SVN with zero contract change and zero taxonomy pollution.

## Full SVN → IR 1.0.0 coverage

| SVN construct | IR 1.0.0 representation | Status |
|---|---|---|
| Global revision (r_n) | `ChangeAtom`, `parents = [prev]` (linear spine) | Stated |
| Repo UUID + revision number | `SourceIdentity { kind: Svn, repo_id, atom_id }` (`model.rs:24,40`) | opaque |
| `svn:author` (may be empty) | `MetadataClaims.author: Option<Identity>` = `None` if empty (`model.rs:110`) | claim |
| `svn:date`, `svn:log` | `MetadataClaims.{author_time, message}` | claim |
| Add / modify / delete path | `PathOp::{Add,Modify,Delete}` (`model.rs:53`) | Stated |
| `copyfrom` copy / move | `RenameHint { status: Stated }` beside literal ops (`model.rs:89`) | Stated |
| `svn:executable` / `svn:special` | `PathOp` `mode: u32` (exec bit / symlink) | Stated |
| Branch by convention (`/branches/x`) | `RefRecord { kind: Branch, status: Derived(ReconstructedBranch) }` | Derived |
| Tag by convention (`/tags/y`) | `RefRecord { kind: Tag, status: Derived(…), params: immutability }` | Derived |
| `svn:mergeinfo` | `DropRecord { class: AdvisoryUnreliable }` (`model.rs:199`), never a parent | dropped-with-record |
| `svn:eol-style`, `svn:keywords` | stored normal-form bytes carried; expansion `DropRecord` (Representation) | dropped-with-record |
| `svn:ignore`, custom revprops | `DropRecord` (Representation / Other) | dropped-with-record |
| `svn:externals` | refused with a named reason (OQ-B) — not represented | refused (N/A) |

**Decoder-level losses that are NOT IR-contract gaps** (record them in the loss boundary; they need no new
IR field, because prikk's target model has no such concept either):

- **Empty directories** — SVN versions them; the file-oriented IR (like git/prikk) has no empty-dir entity.
  A `DropRecord` (Representation) with the path. A published faithfulness boundary (VF-5), not a gap.
- **Directory as a first-class entity** — SVN tracks directory adds/copies/deletes; the IR is path/file
  oriented. A directory copy expands to per-file `PathOp`s (+ `RenameHint`s). No IR change; a decoder
  expansion rule for the handoff.

## Secondary observation (non-blocking; does not affect SVN or the freeze)

The exercise surfaced a latent nuance in the forward-compat *story*, worth recording now that a post-freeze
source has looked hard at the contract. It is **not an SVN issue** (SVN adds no variant) and **not a freeze
change** — flagging it for a separate, unhurried decision.

`version.rs` states the policy as "additive-only within major 1: new optional fields **and new versioned
enum variants** only" (`version.rs:5-8`), and `is_readable` admits any artifact whose **major** is known,
its doc claiming "A newer minor/patch within a known major is readable (additive-only forward
compatibility)" (`version.rs:34-41`). But the decoders `return Err(Error::Decode("unknown … {o}"))` on an
unknown enum tag (e.g. `status.rs:164` `DerivationKind`, `model.rs:281` `SourceKind`, `:572` `RefKind`).
So an **old 1.0.0 reader** handed a **future 1.1 artifact** that uses a new additive variant passes the
version gate, then **fails at decode** on the unknown tag.

I read this as *intended and correct*, not a bug — and consistent with brygge's ethos: silently *skipping*
an unknown derived-marking would suppress honesty (an unread `Derived` mark = a derived thing seen as
absent = manufactured verification, INV-1). So an old reader **should** refuse an artifact it cannot fully
decode, exactly as it refuses an unknown major. The gap is only that the *message* is a generic decode
error rather than a crisp "this artifact uses a newer minor contract than I support."

**Recommendation (separate follow-up, my call as architect to schedule):** (i) correct `is_readable`'s doc
so it no longer promises full forward-readability of newer minors — the version gate admits the major;
decode still, by design, refuses an unknown additive variant; and (ii) optionally make that refusal
minor-aware for a clear message (check the artifact's minor against `CURRENT.minor` and report
"unsupported minor" before hitting the raw tag). Neither touches the wire format, the freeze, or SVN.

## Conclusion

- **SVN fits IR 1.0.0 with no contract change.** The `brygge-decode-svn` build spec may assume the frozen
  types as-is; it introduces **no** new IR field or variant.
- One within-contract decoder choice to carry into the handoff: reconstructed-tag `DerivationKind`
  (recommended: reuse `ReconstructedBranch` + immutability in `params`; defer a first-class
  `ReconstructedTag` until a consumer needs it).
- The freeze's additive-only discipline is demonstrated, not merely asserted, by its first post-freeze
  source: nothing needed adding.
- A separate, non-blocking doc/message refinement to `version.rs`'s forward-compat story is recommended but
  is unrelated to SVN.
