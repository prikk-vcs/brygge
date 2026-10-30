//! Mutation coverage for [`super::check`] (RFC 018 D-6 §6, proof item 5): against one **fixed**, valid
//! input, [`super::check`] must never panic on an `output` built from mutated bytes — however the
//! mutation corrupts the transform record, the atom/ref matching, the marks, or the drops. Whether it
//! then returns `Ok` or `Err` is not asserted here at all: a mutated output is not expected to stay
//! honest, only to be refused (or, on rare luck, accepted) without the checker itself misbehaving. A
//! byte string `from_bytes` itself refuses before `check` is ever reached is `Outcome::Ok` too — this
//! seed targets `check`'s own logic, not the artifact codec, which `crate::artifact::mutation` already
//! covers.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::time::Duration;

use brygge_mutate::harness::{self, Outcome, Seed};

use super::{Edit, check};
use crate::artifact::{from_bytes, to_bytes};
use crate::builder::{AtomDraft, IrBuilder};
use crate::model::{ImportProvenance, MetadataClaims, PathOp, SourceIdentity, SourceKind, Text};
use crate::status::EpistemicStatus;

const QUICK_ITERATIONS: u64 = 250;
const DEEP_FACTOR: u64 = 400;
const TIME_BUDGET: Duration = Duration::from_secs(2);

fn provenance() -> ImportProvenance {
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
        params: BTreeMap::new(),
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

/// The fixed input: two atoms, a ref, so the mutated output has a chain, parents, and a ref for the
/// checker's own matching logic to work over.
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
                message: Some(Text::utf8("root")),
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
            message: Some(Text::utf8("child")),
            ..MetadataClaims::default()
        },
        source: source(2),
        status: EpistemicStatus::Stated,
    })
    .unwrap();
    let ir = b.finish().unwrap();
    from_bytes(&to_bytes(&ir)).unwrap()
}

/// An honest output of `input`, by `Edit` itself: the seed the harness mutates.
fn honest_output_bytes(input: &crate::artifact::Decoded) -> Vec<u8> {
    let mut edit = Edit::begin(input, "mutation-seed", "0", BTreeMap::new());
    edit.atom_mut(0)
        .unwrap()
        .set_message(Some(Text::utf8("root, renamed")));
    let ir = edit.finish().unwrap();
    to_bytes(&ir)
}

#[test]
fn check_on_a_mutated_output_against_a_fixed_input_never_panics() {
    let input = fixed_input();
    let seed = honest_output_bytes(&input);
    let specs = [Seed::generic(&seed)];
    harness::run(
        env!("CARGO_MANIFEST_DIR"),
        "check_on_a_mutated_output_against_a_fixed_input_never_panics",
        &specs,
        QUICK_ITERATIONS,
        DEEP_FACTOR,
        TIME_BUDGET,
        |_seed_index, mutated_bytes| match from_bytes(mutated_bytes) {
            Err(_) => Outcome::Ok,
            Ok(output) => {
                let _ = check(&input, &output, "mutation-seed");
                Outcome::Ok
            }
        },
    );
}
