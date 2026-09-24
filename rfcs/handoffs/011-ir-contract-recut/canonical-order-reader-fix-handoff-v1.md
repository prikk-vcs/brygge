# Handoff — the IR reader's canonical order for refs, drops and flags (a contract-conformance fix)

**Governing:** IR contract 0.2.0 (RFC 011), `docs/src/reference/ir-artifact-format.md` §6. **Crate:**
`brygge-ir` (plus one CLI end-to-end test). **Batch I.** Found by the dev team (request 030 §5), and
reproduced by the architect (review 036). **It lands before C-1.**

## The defect

- **The spec (§6):**
  - refs are strictly ascending by `(name, kind variant)`;
  - drops by `(class variant, what)`;
  - flags by `(kind variant, what)`;
  - "variant" is the variant **number** in the §5 tables.
- **The writer conforms.** `IrBuilder::finish` sorts by `ref_kind_rank`, `loss_class_rank` and
  `flag_kind_rank`, which are exactly those numbers.
- **The reader does not.** `Ir::decode_metadata` (`model.rs`, after the field loop) compares
  `format!("{:?}", …)` strings. Alphabetical order is not variant order:
  - `Other` sorts before `Representation`;
  - `BelowConfidenceFloor` sorts before `ConventionViolation`;
  - `Bookmark` sorts before `Branch`.
- **So brygge writes artifacts its own reader rejects** (`NonCanonical`, `verify` exits 50). Examples: any
  artifact with a `Representation` and an `Other` drop (every CVS repository with branch revisions); any
  artifact with both flag kinds; a ref name used by both a branch and a bookmark.
  - It has been present since `5df4241`, before 0.1.0.

## The fix

1. **One source of truth for the variant number.** Give `RefKind`, `LossClass` and `FlagKind` a
   `pub(crate) fn variant(&self) -> u8`: the §5 table number, the same value their encoder writes.
   - Have the builder's three `*_rank` functions call it, or delete them.
   - The reader's three checks compare `(name, kind.variant())`, `(class.variant(), what)` and
     `(kind.variant(), what)`.
   - No Debug string may take part in any ordering.
2. **Strictness stays: the builder never writes what the reader rejects.**
   - After sorting, `finish` returns `Error::Invariant` if two refs share `(name, variant)`, two drops share
     `(class, what)`, or two flags share `(kind, what)`. Two `RefKind::Other` refs with one name share
     variant 4, so they are a duplicate by the spec.
   - Today such an artifact would be written and then refused on read. Check whether any decoder can
     produce one. If one can, stop and report it, since that is a decoder fix; do not relax the spec.
3. **Audit every other order check in `model.rs` and `artifact.rs`** (ops, copies, atoms, blobs, map keys,
   field tags, and any list inside provenance or source identity) against the builder and the spec. List
   each one in the request with "consistent" or the fix. Fix any other mismatch the same way.

## What does not change

- **The byte format and the contract version (0.2.0).**
- **The writer's output for every input:** it already wrote variant order.
- **Every artifact any brygge version wrote** becomes readable. Show that with the artifact the architect
  reproduced (below): produced by the current build, it now verifies.

## Tests

- **In `brygge-ir`, exhaustive:** for every pair of variants of each of the three enums, in both insertion
  orders, `IrBuilder` → `to_bytes` → `from_bytes` succeeds.
  - A hand-built byte stream with the pair in the wrong variant order is `NonCanonical`.
  - A duplicate key is an `Invariant` from `finish`.
- **CLI end to end:** a CVS repository (a hand-written `,v` is enough) with one branch revision, decoded
  **without** `--reconstruct-refs`. `brygge verify` passes `integrity`: exit 0 or 10, not 50. The
  architect's reproduction was a file `a.txt,v`, with `1.1` then `1.2` on the trunk, `BR:1.2.0.2`, and one
  revision `1.2.2.1`.
- **The existing suites** pass unchanged.

## CHANGELOG `[Unreleased]`, Fixed

"An artifact containing both kinds of flag, or drops of different classes (for example any CVS repository
with branch revisions), failed its own `verify` integrity check (exit 50), because the reader compared
names instead of the specified variant order. The reader now follows the specification. Artifacts already
written by any version are valid and now verify."

## Review request

`.git-exclude/review-request/032-ir-canonical-order-reader.md`, with the audit table. After approval, commit
and push to `main`, and append CI. The architect then decides with the owner whether to cut a 0.2.1 patch
release.
