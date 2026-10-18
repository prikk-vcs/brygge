//! Mutation tests for `verify`'s internal checks (RFC 014 D-1, review 061 F-3): `brygge verify` on a
//! received artifact is exactly what a user runs on an artifact someone else produced, so the checks it
//! runs (structure, replay, derivations, source-invariants, provenance, loss-boundary) are untrusted-
//! input surface too — the same artifact bytes `brygge-ir`'s own mutation module already fuzzes against
//! `from_bytes`, carried one step further into the checks the CLI actually runs on a decoded artifact.
//!
//! The resigned-mutation approach, as `brygge-ir`'s module uses: mutate only the body (everything after
//! the digest), then recompute a digest that matches the mutated body, so the mutation reaches the
//! checks below instead of being caught by the (already separately fuzzed) digest check first.

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
use brygge_mutate::ops::{Encoding, InterestingPosition};
use sha2::{Digest, Sha256};

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
    let blob = b.add_blob(b"hello world".to_vec());
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
    let child_blob = b.add_blob(b"second revision text".to_vec());
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

/// Minimal unsigned LEB128 skip: returns the position right after the varint starting at `pos`.
fn skip_uvarint(data: &[u8], mut pos: usize) -> usize {
    while data.get(pos).is_some_and(|b| b & 0x80 != 0) {
        pos += 1;
    }
    pos + 1
}

/// Split `brygge_ir::to_bytes`'s output into `(prefix, body)` exactly as `from_bytes` reads it (RFC 011
/// §2.3): `magic(8) · format(1) · version(uvarint × 3) · digest(32) · body`. `prefix` is
/// `magic ++ format ++ version` (what the digest is computed over, together with `body`); `body` is
/// everything after the digest.
fn split_header(bytes: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut pos = 9; // magic(8) + format(1)
    for _ in 0..3 {
        pos = skip_uvarint(bytes, pos);
    }
    let prefix = bytes.get(..pos).expect("header present").to_vec();
    let body = bytes.get(pos + 32..).expect("digest present").to_vec();
    (prefix, body)
}

/// Reassemble `(prefix, mutated_body)` with a digest that matches `prefix[9..] ++ mutated_body` — a
/// mutation that is wrong only in the body, correctly signed, exactly what the checks below exist to
/// catch (the already-fuzzed digest check, in `brygge-ir`'s own module, would otherwise shadow them).
fn resign(prefix: &[u8], body: &[u8]) -> Vec<u8> {
    let version_bytes = prefix.get(9..).unwrap_or(&[]);
    let mut hasher = Sha256::new();
    hasher.update(version_bytes);
    hasher.update(body);
    let digest: [u8; 32] = hasher.finalize().into();
    let mut out = Vec::with_capacity(prefix.len() + 32 + body.len());
    out.extend_from_slice(prefix);
    out.extend_from_slice(&digest);
    out.extend_from_slice(body);
    out
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

fn check(prefix: &[u8], mutated_body: &[u8]) -> Outcome {
    let full = resign(prefix, mutated_body);
    match brygge_ir::from_bytes(&full) {
        Ok(decoded) => run_internal_checks(&decoded.ir),
        Err(_) => Outcome::Ok,
    }
}

fn meta_len_position(body: &[u8]) -> InterestingPosition {
    let len = skip_uvarint(body, 0);
    InterestingPosition {
        offset: 0,
        encoding: Encoding::Uvarint { len },
        ceiling: Some((body.len() - len) as u64),
    }
}

#[test]
fn verifys_internal_checks_never_panic_on_a_mutated_but_correctly_signed_artifact() {
    let full = brygge_ir::to_bytes(&seed_ir());
    let (prefix, body) = split_header(&full);
    // Sanity: the unmutated, resigned body decodes and every check at least runs (does not panic) —
    // confirms the split/resign machinery is correct before fuzzing it.
    assert!(brygge_ir::from_bytes(&resign(&prefix, &body)).is_ok());

    let position = meta_len_position(&body);
    let positions = [position];
    let seeds = [Seed::new(&body, &positions)];
    harness::run(
        env!("CARGO_MANIFEST_DIR"),
        "verifys_internal_checks_never_panic_on_a_mutated_but_correctly_signed_artifact",
        &seeds,
        QUICK_ITERATIONS,
        DEEP_FACTOR,
        TIME_BUDGET,
        |_seed_index, mutated_body| check(&prefix, mutated_body),
    );
}
