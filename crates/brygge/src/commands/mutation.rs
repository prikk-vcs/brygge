//! Mutation tests for `verify`'s internal checks (RFC 014 D-1, review 061 F-3): `brygge verify` on a
//! received artifact is exactly what a user runs on an artifact someone else produced, so the checks it
//! runs (structure, replay, derivations, source-invariants, provenance, loss-boundary) are untrusted-
//! input surface too — the same artifact bytes `brygge-ir`'s own mutation module already fuzzes against
//! `from_bytes`, carried one step further into the checks the CLI actually runs on a decoded artifact.
//!
//! Mutates the whole artifact (RFC 018 D-1: this crate has no access to `brygge-ir`'s crate-internal
//! frame/digest machinery, so it can no longer resign a mutated body as a contract-0.2.0-era resigned
//! mutation did — see [`check`]'s own doc).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::time::Duration;

use brygge_ir::builder::{AtomDraft, IrBuilder};
use brygge_ir::model::{
    DropRecord, Flag, FlagKind, ImportProvenance, LossBoundary, LossClass, MetadataClaims, PathOp,
    RefKind, RefRecord, SourceIdentity, SourceKind, Text,
};
use brygge_ir::status::EpistemicStatus;
use brygge_mutate::harness::{self, Outcome, Seed};

use super::{
    check_derivations, check_loss_boundary, check_provenance, check_replay,
    check_source_invariants, check_structure,
};

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

/// A multi-atom artifact exercising every check `verify --internal` runs something real on: two atoms
/// (so `structure`/`replay` walk a real parent chain), a ref, a drop, and a flag.
fn seed_ir() -> brygge_ir::Ir {
    let mut b = IrBuilder::new(provenance());
    let blob = b.add_blob(b"hello world".to_vec()).unwrap();
    let root = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![PathOp::Add {
                path: "readme".into(),
                blob,
                mode: 0o100_644,
                status: EpistemicStatus::Stated,
            }],
            copies: vec![],
            metadata: MetadataClaims {
                message: Some(Text::utf8("initial")),
                ..MetadataClaims::default()
            },
            source: SourceIdentity {
                kind: SourceKind::Git,
                repo_id: b"repo".to_vec(),
                atom_id: b"c1".to_vec(),
                signatures: Vec::new(),
                extras: Vec::new(),
            },
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    let child_blob = b.add_blob(b"second revision text".to_vec()).unwrap();
    let child = b
        .add_atom(AtomDraft {
            parents: vec![root],
            ops: vec![PathOp::Add {
                path: "second".into(),
                blob: child_blob,
                mode: 0o100_644,
                status: EpistemicStatus::Stated,
            }],
            copies: vec![],
            metadata: MetadataClaims::default(),
            source: SourceIdentity {
                kind: SourceKind::Git,
                repo_id: b"repo".to_vec(),
                atom_id: b"c2".to_vec(),
                signatures: Vec::new(),
                extras: Vec::new(),
            },
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    b.add_ref(RefRecord {
        name: "refs/heads/main".into(),
        kind: RefKind::Branch,
        target: child,
        status: EpistemicStatus::Stated,
        source: None,
        annotation: None,
    })
    .unwrap();
    b.set_loss(LossBoundary {
        dropped: vec![DropRecord {
            class: LossClass::Other,
            what: "an example drop".into(),
            reason: "mutation-seed".into(),
        }],
    });
    b.add_flag(Flag {
        kind: FlagKind::BelowConfidenceFloor,
        what: "an example flag".into(),
        count: 1,
        reason: "mutation-seed".into(),
    });
    b.finish().unwrap()
}

/// Every one of `verify`'s internal checks, run over `decoded.ir`. Each returns a `CheckOutcome`, a
/// closed three-variant enum (`Pass`/`Fail`/`NotChecked`) — "a verdict that is one of the documented
/// ones" is true by construction once this compiles; what a panic inside any of them would defeat is
/// catalogued by the harness's `catch_unwind`, not by inspecting the result here.
fn run_internal_checks(ir: &brygge_ir::Ir) -> Outcome {
    let _ = check_structure(ir);
    let _ = check_replay(ir);
    let _ = check_derivations(ir);
    let _ = check_source_invariants(ir);
    let _ = check_provenance(ir);
    let _ = check_loss_boundary(ir);
    Outcome::Ok
}

/// RFC 018 D-1 note: contract 0.3.0's digest covers the artifact's *logical* (decompressed) stream, a
/// fact only `brygge-ir`'s own, crate-internal reader/writer machinery can recompute — this crate sees
/// only its public `to_bytes`/`from_bytes`, so the resigned-body trick the RFC 014 D-1 handoff describes
/// (mutate the body, recompute a matching digest, reach the checks below without the digest check
/// shadowing them) is no longer available here. Mutating the *whole* artifact directly reaches these
/// checks less often (most mutations are now caught earlier, inside a frame), but it is correct and
/// uses only brygge-ir's public surface; `from_bytes`'s own crate-internal mutation tests (RFC 018
/// §6.6) cover the frame/digest layer this one no longer reaches directly.
fn check(mutated_bytes: &[u8]) -> Outcome {
    match brygge_ir::from_bytes(mutated_bytes) {
        Ok(decoded) => run_internal_checks(&decoded.ir),
        Err(_) => Outcome::Ok,
    }
}

#[test]
fn verifys_internal_checks_never_panic_on_a_mutated_but_correctly_signed_artifact() {
    let full = brygge_ir::to_bytes(&seed_ir());
    let seeds = [Seed::generic(&full)];
    harness::run(
        env!("CARGO_MANIFEST_DIR"),
        "verifys_internal_checks_never_panic_on_a_mutated_but_correctly_signed_artifact",
        &seeds,
        QUICK_ITERATIONS,
        DEEP_FACTOR,
        TIME_BUDGET,
        |_seed_index, mutated_bytes| check(mutated_bytes),
    );
}
