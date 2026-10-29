//! Mutation tests for `verify`'s internal checks (RFC 014 D-1, review 061 F-3): `brygge verify` on a
//! received artifact is exactly what a user runs on an artifact someone else produced, so the checks it
//! runs (structure, replay, derivations, source-invariants, provenance, loss-boundary) are untrusted-
//! input surface too — the same artifact bytes `brygge-ir`'s own mutation module already fuzzes against
//! `from_bytes`, carried one step further into the checks the CLI actually runs on a decoded artifact.
//!
//! **Review 099 R-2:** this crate has no access to `brygge-ir`'s crate-internal frame/digest machinery,
//! so the resigned-mutation approach is reimplemented here against the *published* format page
//! (`docs/src/reference/ir-artifact-format.md`) instead — parsing a contract-0.3.0 container's metadata
//! frames and blob section with `miniz_oxide` directly (already in this crate's dependency closure
//! through `brygge-ir`; added here only as a dev-dependency, no new crate). This doubles as a check
//! that the format page itself suffices to build a reader from (PU-3): if it did not, this module could
//! not have been written from it alone.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::time::Duration;

use brygge_ir::builder::{AtomDraft, IrBuilder};
use brygge_ir::model::{
    ChangedPart, DropRecord, Flag, FlagKind, ImportProvenance, LossBoundary, LossClass,
    MetadataClaims, PathOp, RefKind, RefRecord, SourceIdentity, SourceKind, Text, TransformRecord,
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
    let step = b.add_transform(TransformRecord {
        name: "renumber".into(),
        version: "1.0.0".into(),
        params: BTreeMap::new(),
        consumed: [0x42; 32],
    });
    let child = b
        .add_atom(AtomDraft {
            parents: vec![root],
            ops: vec![PathOp::Add {
                path: "second".into(),
                blob: child_blob,
                mode: 0o100_644,
                status: EpistemicStatus::Transformed {
                    step: step as u64,
                    changed: vec![ChangedPart::Mode],
                    prior: Box::new(EpistemicStatus::Stated),
                },
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
            status: EpistemicStatus::Transformed {
                step: step as u64,
                changed: vec![ChangedPart::Ops],
                prior: Box::new(EpistemicStatus::Stated),
            },
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
        dropped: vec![
            DropRecord {
                class: LossClass::Other,
                what: "an example drop".into(),
                reason: "mutation-seed".into(),
                step: None,
            },
            DropRecord {
                class: LossClass::Transform,
                what: "an example transform drop".into(),
                reason: "mutation-seed".into(),
                step: Some(step as u64),
            },
        ],
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

// ---- a contract-0.3.0 reader/resigner, built against the published format page alone (review 099 R-2) --

/// Read a `uvarint` from `bytes` at `*pos`, advancing `*pos` past it. Minimal LEB128
/// (`docs/src/reference/ir-artifact-format.md` §1) — overlong/overflow are not re-checked here: this
/// module only ever reads bytes `to_bytes` itself just wrote, before mutating a copy of them.
fn read_uvarint(bytes: &[u8], pos: &mut usize) -> u64 {
    let mut result = 0u64;
    let mut shift = 0u32;
    loop {
        let byte = bytes[*pos];
        *pos += 1;
        result |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    result
}

fn write_uvarint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
}

/// Inflate one frame's stored bytes per the format page's §3 codec rule (`0`: stored, as is; `1`:
/// DEFLATE, raw). A whole-buffer inflate, bounded by `raw_len` — fine here: this module only ever
/// inflates a frame `to_bytes` itself just produced (at most a few KiB for a mutation seed's own
/// metadata/blobs), never an artifact from an untrusted source (`brygge-ir`'s own streamed, bounded
/// inflate — review 099 R-1 — is what a real untrusted artifact goes through, in `verify` itself).
fn inflate_frame(codec: u64, stored: &[u8], raw_len: u64) -> Vec<u8> {
    match codec {
        0 => stored.to_vec(),
        1 => miniz_oxide::inflate::decompress_to_vec_with_limit(stored, raw_len as usize)
            .expect("a frame this module itself produced inflates cleanly"),
        other => panic!("unknown frame codec {other}"),
    }
}

/// Read one frame (`uvarint(codec) ‖ uvarint(raw_len) ‖ uvarint(stored_len) ‖ stored bytes`), advancing
/// `*pos` past it, and return its inflated bytes.
fn read_frame(bytes: &[u8], pos: &mut usize) -> Vec<u8> {
    let codec = read_uvarint(bytes, pos);
    let raw_len = read_uvarint(bytes, pos);
    let stored_len = read_uvarint(bytes, pos) as usize;
    let stored = &bytes[*pos..*pos + stored_len];
    *pos += stored_len;
    inflate_frame(codec, stored, raw_len)
}

/// Write one frame, `Codec::Stored` only (this module only ever resigns, never recompresses — the
/// mutated metadata is small, so there is no compression ratio to preserve either way).
fn write_stored_frame(out: &mut Vec<u8>, raw: &[u8]) {
    write_uvarint(out, 0); // codec: stored
    write_uvarint(out, raw.len() as u64);
    write_uvarint(out, raw.len() as u64);
    out.extend_from_slice(raw);
}

/// Parsed from a contract-0.3.0 artifact's header (`docs/src/reference/ir-artifact-format.md` §3):
/// `version_bytes` (for resigning), the metadata's own inflated bytes, and `tail` — everything from the
/// blob table onward, unparsed and untouched (this module mutates metadata only; review 099 R-3 is
/// `brygge-ir`'s own crate-internal module, which also covers the blob table and blob content).
struct Parsed {
    version_bytes: Vec<u8>,
    meta_bytes: Vec<u8>,
    tail: Vec<u8>,
}

fn parse(bytes: &[u8]) -> Parsed {
    assert_eq!(&bytes[..8], b"BRYGGEIR");
    assert_eq!(bytes[8], 2, "format byte");
    let mut pos = 9usize;
    let version_start = pos;
    let major = read_uvarint(bytes, &mut pos);
    let minor = read_uvarint(bytes, &mut pos);
    let _patch = read_uvarint(bytes, &mut pos);
    assert_eq!(
        (major, minor),
        (0, 3),
        "this module reads contract 0.3.0 only"
    );
    let version_bytes = bytes[version_start..pos].to_vec();
    pos += 32; // the stored digest, not re-read here (recomputed on resign)

    let nframes_meta = read_uvarint(bytes, &mut pos);
    let mut meta_bytes = Vec::new();
    for _ in 0..nframes_meta {
        meta_bytes.extend_from_slice(&read_frame(bytes, &mut pos));
    }

    let tail = bytes[pos..].to_vec();
    Parsed {
        version_bytes,
        meta_bytes,
        tail,
    }
}

/// Recompute the logical-stream digest (`ir-artifact-format.md` §3) for `(version_bytes, meta_bytes,
/// tail)` — `tail`'s own blob table is logical already (stored raw, never compressed); its blob
/// frames are inflated here, read-only, purely to hash their real content.
fn resign(version_bytes: &[u8], meta_bytes: &[u8], tail: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(version_bytes);
    let mut meta_len_bytes = Vec::new();
    write_uvarint(&mut meta_len_bytes, meta_bytes.len() as u64);
    hasher.update(&meta_len_bytes);
    hasher.update(meta_bytes);

    let mut pos = 0usize;
    let nblobs = read_uvarint(tail, &mut pos);
    let mut table_start_to_end = Vec::new();
    write_uvarint(&mut table_start_to_end, nblobs);
    let mut lens = Vec::with_capacity(nblobs as usize);
    for _ in 0..nblobs {
        let id_start = pos;
        pos += 32;
        let len = read_uvarint(tail, &mut pos);
        table_start_to_end.extend_from_slice(&tail[id_start..id_start + 32]);
        let mut len_bytes = Vec::new();
        write_uvarint(&mut len_bytes, len);
        table_start_to_end.extend_from_slice(&len_bytes);
        lens.push(len);
    }
    hasher.update(&table_start_to_end);

    let nframes_blob = read_uvarint(tail, &mut pos);
    for _ in 0..nframes_blob {
        hasher.update(read_frame(tail, &mut pos));
    }
    hasher.finalize().into()
}

/// Reassemble a full artifact from `(version_bytes, mutated meta_bytes, tail)`, with the metadata
/// re-framed fresh, `Codec::Stored` (fixed 1 MiB chunking — trivial for this module's small seeds, so
/// always one frame), and `tail` copied verbatim: the blob table and blob frames never change.
fn reassemble(version_bytes: &[u8], meta_bytes: &[u8], tail: &[u8]) -> Vec<u8> {
    let digest = resign(version_bytes, meta_bytes, tail);
    let mut out = b"BRYGGEIR".to_vec();
    out.push(2);
    out.extend_from_slice(version_bytes);
    out.extend_from_slice(&digest);
    write_uvarint(&mut out, 1); // one metadata frame
    write_stored_frame(&mut out, meta_bytes);
    out.extend_from_slice(tail);
    out
}

fn check(version_bytes: &[u8], tail: &[u8], mutated_meta_bytes: &[u8]) -> Outcome {
    let full = reassemble(version_bytes, mutated_meta_bytes, tail);
    match brygge_ir::from_bytes(&full) {
        Ok(decoded) => run_internal_checks(&decoded.ir),
        Err(_) => Outcome::Ok,
    }
}

/// The metadata's own leading field-count `uvarint` is the one position every seed can locate without
/// re-implementing the rest of the canonical codec: it is always the first byte of the `Ir` record.
fn meta_field_count_position(meta_bytes: &[u8]) -> InterestingPosition {
    let mut pos = 0usize;
    let before = pos;
    let _ = read_uvarint(meta_bytes, &mut pos);
    InterestingPosition {
        offset: 0,
        encoding: Encoding::Uvarint { len: pos - before },
        ceiling: None,
    }
}

#[test]
fn verifys_internal_checks_never_panic_on_a_mutated_but_correctly_signed_artifact() {
    let full = brygge_ir::to_bytes(&seed_ir());
    let parsed = parse(&full);

    // Sanity: the unmutated, resigned artifact decodes — confirms the parse/resign machinery, built
    // from the published page alone, is correct before fuzzing it.
    let resigned = reassemble(&parsed.version_bytes, &parsed.meta_bytes, &parsed.tail);
    assert!(brygge_ir::from_bytes(&resigned).is_ok());

    let position = meta_field_count_position(&parsed.meta_bytes);
    let positions = [position];
    let seeds = [Seed::new(&parsed.meta_bytes, &positions)];
    harness::run(
        env!("CARGO_MANIFEST_DIR"),
        "verifys_internal_checks_never_panic_on_a_mutated_but_correctly_signed_artifact",
        &seeds,
        QUICK_ITERATIONS,
        DEEP_FACTOR,
        TIME_BUDGET,
        |_seed_index, mutated_meta_bytes| {
            check(&parsed.version_bytes, &parsed.tail, mutated_meta_bytes)
        },
    );
}
