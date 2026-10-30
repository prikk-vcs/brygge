//! Mutation coverage for [`super::check`] (RFC 018 D-6 §6, proof item 5; review 103 P-2): against one
//! **fixed**, valid input, [`super::check`] must never panic on an `output` built from mutated bytes —
//! however the mutation corrupts the transform record, the atom/ref matching, the marks, or the drops.
//! Whether it then returns `Ok` or `Err` is not asserted here at all: a mutated output is not expected
//! to stay honest, only to be refused (or, on rare luck, accepted) without the checker itself
//! misbehaving.
//!
//! **Review 103 P-2 found one wall and asked for a fix; building it surfaced two more. All three are
//! disclosed in review request 063's "Review 103 fixes" section, in the order found:**
//!
//! 1. **The digest (the wall the review itself diagnosed).** A first version of this seed mutated the
//!    output's raw bytes generically (`Seed::generic`, no re-signing), so almost every mutation was
//!    refused by the digest check inside `from_bytes` before `check` was ever reached — 100,000 deep
//!    iterations in 0.23 s was the symptom, not a sign of thorough coverage. Fixed by reusing
//!    [`crate::artifact::mutation::decompose`]/[`crate::artifact::mutation::reassemble_v3`] (the same
//!    pair `artifact::mutation`'s own "resigned, on the logical stream" strategy uses): mutate only the
//!    output's logical metadata, then re-frame it `Codec::Stored` and recompute a *matching* digest.
//! 2. **Every atom's own self-verifying id.** Removing the digest wall alone still left the
//!    reached-`check` share near zero, because `ChangeAtom::decode` recomputes every atom's own
//!    `AtomId` from its body and refuses a mismatch (`model.rs`, "atom id does not match its
//!    contents") — and almost the entire metadata byte range is *inside* some atom's own body (fields
//!    2–7 of `ChangeAtom`), so almost any byte touched there trips this check before `check` ever runs.
//!    Confirmed directly: a sweep flipping one bit at a time across every byte of a real (re-signed)
//!    metadata buffer reached `from_bytes`'s `Ok` path only ~11% of the time, nearly every failure this
//!    one check. Fixed by confining the mutated region to everything in the metadata *after* field 1
//!    (the atoms list) — refs, provenance, drops, flags, and the transform record itself, none of which
//!    carries a self-verifying content hash, and which is also exactly the territory `check` itself
//!    inspects most directly. Field 1's own bytes are carried through unmutated in every iteration (as
//!    are the new transform record's own `name`/`version`, grammar- and non-emptiness-validated —
//!    `after_name_and_version`).
//! 3. **The shared harness's own operator mix, which this module cannot change.** Even past both walls
//!    above, a third of `ops::apply_generic`'s own variants change the mutated buffer's *length*
//!    (insert/delete/duplicate/splice a run), which desyncs this length-prefixed format's framing
//!    almost unconditionally, wherever they land; `apply_truncate` is chosen on top of that about one
//!    time in eight. `harness::mutate` also chains 1–3 such operators per iteration, so even a single
//!    operator's own success rate gets squared or cubed. The measured result, after fixes 1 and 2 and
//!    with every free-form field (messages, identities, provenance/step `params`) padded long so a
//!    byte-preserving mutation has more harmless territory to land on: **~45–46% of iterations reach
//!    `check`** — a dramatic improvement over wall 2's ~11% single-operator ceiling (let alone wall 1's
//!    near-zero), but short of a literal majority. This module's own assertion guards against a
//!    regression back toward either of the first two walls (reached share collapsing toward zero), not
//!    a claim of "most" in the strict sense.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::cell::Cell;
use std::collections::BTreeMap;
use std::time::Duration;

use brygge_mutate::harness::{self, Outcome, Seed};
use brygge_mutate::ops::{Encoding, InterestingPosition};

use super::{Edit, check};
use crate::artifact::mutation::{decompose, reassemble_v3};
use crate::artifact::{from_bytes, to_bytes};
use crate::builder::{AtomDraft, IrBuilder};
use crate::model::{
    Identity, ImportProvenance, MetadataClaims, PathOp, SourceIdentity, SourceKind, Text,
};
use crate::status::EpistemicStatus;

const QUICK_ITERATIONS: u64 = 500;
const DEEP_FACTOR: u64 = 400;
const TIME_BUDGET: Duration = Duration::from_secs(2);

/// Padding, free of any canonical-form constraint on its own content — used throughout this fixture
/// (provenance params, the transform's own params) to maximize the share of mutable bytes that sit in
/// territory a mutation can land on without breaking anything structural (the module doc's second
/// wall).
fn padding(label: &str) -> String {
    format!(
        "{label}-padding-{}",
        "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod ".repeat(40)
    )
}

fn provenance() -> ImportProvenance {
    let mut params = BTreeMap::new();
    params.insert("padding".to_string(), padding("provenance"));
    ImportProvenance {
        source: SourceIdentity {
            kind: SourceKind::Git,
            repo_id: b"repo".to_vec(),
            atom_id: b"root".to_vec(),
            signatures: Vec::new(),
            extras: Vec::new(),
        },
        brygge_version: "0.1.0".into(),
        decoder: "brygge-decode-git".into(),
        decoder_version: "0.1.0".into(),
        params,
    }
}

fn source(n: u8) -> SourceIdentity {
    SourceIdentity {
        kind: SourceKind::Git,
        repo_id: b"repo".to_vec(),
        atom_id: vec![n],
        signatures: Vec::new(),
        extras: Vec::new(),
    }
}

fn long_text(label: &str) -> Text {
    Text::utf8(&format!(
        "{label}: lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor \
         incididunt ut labore et dolore magna aliqua, {label} again for good measure."
    ))
}

fn long_identity(label: &str) -> Identity {
    Identity {
        name: Text::utf8(&format!("{label} Long Padding Name For Mutation Coverage")),
        email: Some(Text::utf8(&format!(
            "{label}.long.padding.email.for.mutation.coverage@example.com"
        ))),
    }
}

/// The fixed input: two atoms, so the mutated output has a chain and parents for the checker's own
/// matching logic to work over. Author/committer/message are padded long (see the module doc's second
/// wall) — harmless here, since field 1 (which holds them) is carried through unmutated either way, but
/// kept for symmetry with the output fixture below.
fn fixed_input() -> crate::artifact::Decoded {
    let mut b = IrBuilder::new(provenance());
    let blob = b.add_blob(b"hello".to_vec()).unwrap();
    let root = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![PathOp::Add {
                path: "a".into(),
                blob,
                mode: 0o100_644,
                status: EpistemicStatus::Stated,
            }],
            copies: vec![],
            metadata: MetadataClaims {
                message: Some(long_text("root")),
                author: Some(long_identity("root-author")),
                committer: Some(long_identity("root-committer")),
                ..MetadataClaims::default()
            },
            source: source(1),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    let child_blob = b.add_blob(b"hello, again".to_vec()).unwrap();
    b.add_atom(AtomDraft {
        parents: vec![root],
        ops: vec![PathOp::Modify {
            path: "a".into(),
            blob: child_blob,
            mode: 0o100_644,
            status: EpistemicStatus::Stated,
        }],
        copies: vec![],
        metadata: MetadataClaims {
            message: Some(long_text("child")),
            author: Some(long_identity("child-author")),
            committer: Some(long_identity("child-committer")),
            ..MetadataClaims::default()
        },
        source: source(2),
        status: EpistemicStatus::Stated,
    })
    .unwrap();
    let ir = b.finish().unwrap();
    from_bytes(&to_bytes(&ir)).unwrap()
}

/// An honest output of `input`, by `Edit` itself: the seed the harness mutates (decomposed below, never
/// fed to the harness as raw bytes).
fn honest_output_bytes(input: &crate::artifact::Decoded) -> Vec<u8> {
    let mut step_params = BTreeMap::new();
    step_params.insert("padding".to_string(), padding("step"));
    let mut edit = Edit::begin(input, "mutation-seed", "0", step_params);
    edit.atom_mut(0)
        .unwrap()
        .set_message(Some(long_text("root, renamed")));
    let ir = edit.finish().unwrap();
    to_bytes(&ir)
}

/// Walks `buf`'s own record framing far enough to locate field `target_id`'s own value range,
/// returning `(value_start, value_len)` — `None` if absent. `has_leading_count` is `true` for a whole
/// metadata record (`uvarint(field_count)` then each field), `false` for the raw remainder after an
/// earlier field has already been consumed (no count of its own — it is simply fields until the buffer
/// ends).
fn locate_field(buf: &[u8], target_id: u64, has_leading_count: bool) -> Option<(usize, usize)> {
    let mut r = crate::canon::CanonReader::new(buf);
    let total = buf.len();
    let count = if has_leading_count {
        r.uvarint().ok()?
    } else {
        u64::MAX
    };
    let mut seen = 0u64;
    while (has_leading_count && seen < count) || (!has_leading_count && !r.is_empty()) {
        let tag = r.uvarint().ok()?;
        let id = tag >> 1;
        let len = r.uvarint_len().ok()?;
        let value_start = total - r.remaining();
        let _ = r.raw(len).ok()?;
        seen += 1;
        if id == target_id {
            return Some((value_start, len));
        }
    }
    None
}

/// The new transform record's own `consumed` digest (field 4 of `TransformRecord`, inside the one item
/// of field 6's list, within `tail`) — the one value `check`'s own rule 1 compares directly against the
/// input's own digest ("the new transform record's consumed digest does not match the input artifact's
/// own digest"). Declared as the *first 8 bytes* of its 32 (`Encoding::Binary` only supports widths up
/// to 8 — this crate's `canon` module has no 32-byte binary boundary primitive), which is enough to
/// drive it to `0`/`u64::MAX`/etc. **Unlike a count field, `consumed` carries no canonical-form
/// constraint of its own at decode time** (it is 32 raw bytes, read and trusted as given) — so every
/// boundary value here still reaches `check`, instead of desyncing the record's own framing the way a
/// wrong field or list count always does. This is what makes it the one position in this seed that
/// contributes positively to "most of them reach `check`" (review 103 P-2), rather than against it.
fn transform_consumed_position(tail: &[u8]) -> Option<InterestingPosition> {
    // `tail` here starts right after the transform record's own `name`/`version` (see
    // `after_name_and_version`), so what remains of this one record — `params` (optional) then
    // `consumed` — is exactly a bare field sequence with no count of its own to skip first.
    let (consumed_start, consumed_len) = locate_field(tail, 4, false)?;
    debug_assert_eq!(
        consumed_len, 32,
        "TransformRecord.consumed is always 32 bytes"
    );
    Some(InterestingPosition {
        offset: consumed_start,
        encoding: Encoding::Binary {
            width: 8,
            big_endian: true,
        },
        ceiling: None,
    })
}

/// Where, within `tail`, the new transform record's own `name`/`version` fields (1–2, grammar- and
/// non-emptiness-validated at decode time) end and `params`/`consumed` (3–4, unconstrained) begin —
/// `None` if field 6 is absent. Extending the fixed prefix through this point (rather than stopping
/// after field 1/atoms alone) keeps the one other meaningfully decode-fragile field out of the mutated
/// region too, without hand-rolling a second splice for it.
fn after_name_and_version(tail: &[u8]) -> Option<usize> {
    let (list_start, _) = locate_field(tail, 6, false)?;
    let list_value = tail.get(list_start..)?;
    let mut r = crate::canon::CanonReader::new(list_value);
    let _count = r.uvarint().ok()?;
    let item_start = list_value.len() - r.remaining();
    let item_bytes = list_value.get(item_start..)?;
    let (version_start, version_len) = locate_field(item_bytes, 2, true)?;
    Some(list_start + item_start + version_start + version_len)
}

#[test]
fn check_on_a_mutated_output_against_a_fixed_input_never_panics() {
    let input = fixed_input();
    let seed_bytes = honest_output_bytes(&input);
    let decomposed = decompose(&seed_bytes);
    let meta = &decomposed.meta_bytes;

    // Field 1 (atoms) is content-hashed into every atom's own stored `AtomId`; any mutation inside it
    // is refused by `ChangeAtom::decode` before `check` could ever run, no matter how the rest of the
    // output is shaped (the module doc's "second wall"). It is carried through unmutated; mutation is
    // confined to everything after it (and, within that, after the transform record's own `name`/
    // `version`, the one other decode-fragile field left — see `after_name_and_version`).
    let (atoms_start, atoms_len) =
        locate_field(meta, 1, true).expect("the fixture always has atoms");
    let after_atoms = atoms_start + atoms_len;
    let tail_after_atoms = meta.get(after_atoms..).unwrap();
    let extra = after_name_and_version(tail_after_atoms).unwrap_or(0);
    let tail_start = after_atoms + extra;
    let fixed_prefix = meta.get(..tail_start).unwrap().to_vec();
    let tail = meta.get(tail_start..).unwrap().to_vec();

    let position = transform_consumed_position(&tail);
    let positions: Vec<InterestingPosition> = position.into_iter().collect();
    let specs = [Seed::new(&tail, &positions)];

    let total: Cell<u64> = Cell::new(0);
    let reached_check: Cell<u64> = Cell::new(0);

    harness::run(
        env!("CARGO_MANIFEST_DIR"),
        "check_on_a_mutated_output_against_a_fixed_input_never_panics",
        &specs,
        QUICK_ITERATIONS,
        DEEP_FACTOR,
        TIME_BUDGET,
        |_seed_index, mutated_tail| {
            total.set(total.get() + 1);
            let mut full_meta = fixed_prefix.clone();
            full_meta.extend_from_slice(mutated_tail);
            let full = reassemble_v3(
                &decomposed.version_bytes,
                &full_meta,
                &decomposed.table_bytes,
                &decomposed.blob_bytes,
            );
            match from_bytes(&full) {
                Err(_) => Outcome::Ok,
                Ok(output) => {
                    reached_check.set(reached_check.get() + 1);
                    let _ = check(&input, &output, "mutation-seed");
                    Outcome::Ok
                }
            }
        },
    );

    let total = total.get();
    let reached = reached_check.get();
    assert!(total > 0, "the harness ran zero iterations");
    // Not literally "most" (a strict majority) — see the module doc's third wall: the shared harness's
    // own chain length (1–3 operators) and operator mix (a third of its "generic" variants change the
    // buffer's length, which desyncs this length-prefixed format's framing almost unconditionally,
    // regardless of where they land) caps the reachable share measurably under 50% however this seed's
    // own mutable region is shaped. This threshold guards against a regression back toward either of
    // the first two walls (next to nothing reaching `check` at all), not a claim of a literal majority.
    assert!(
        reached * 20 >= total * 7,
        "only {reached}/{total} ({:.1}%) mutated outputs reached `check` — expected at least 35%",
        100.0 * reached as f64 / total as f64
    );
}
