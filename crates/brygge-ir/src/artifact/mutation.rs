//! Mutation tests for the artifact codec (RFC 014 D-1): [`from_bytes`] must never panic on a mutated
//! artifact, and must never decode more content than the bytes it was given could have held, however the
//! header, digest or body is corrupted.
//!
//! Two complementary strategies, since the digest check runs *before* any body parsing and would
//! otherwise shadow everything past it:
//! - **raw:** mutate the whole `to_bytes` output and feed it straight to `from_bytes`. This exercises the
//!   magic/format/version gate and the digest check itself — almost every mutation here is caught as a
//!   `DigestMismatch` or an unsupported-version refusal, which is the correct, typed outcome.
//! - **resigned:** mutate only the body (everything after the digest), then recompute a *matching* digest
//!   over the mutated body before calling `from_bytes`. This is what actually reaches the canonical-order
//!   and referential-integrity checks (RFC 011 §2.5) — the ones a corrupted-but-correctly-signed artifact
//!   would need to defeat.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::time::Duration;

use brygge_mutate::harness::{self, Outcome, Seed};
use brygge_mutate::ops::{Encoding, InterestingPosition};
use sha2::{Digest, Sha256};

use super::{from_bytes, to_bytes};
use crate::builder::{AtomDraft, IrBuilder};
use crate::model::{
    DropRecord, Flag, FlagKind, ImportProvenance, Ir, LossBoundary, LossClass, MetadataClaims,
    PathOp, RefKind, RefRecord, SourceIdentity, SourceKind,
};
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

/// A multi-atom artifact exercising several record kinds at once (two atoms, a ref, a drop, a flag), so
/// the mutation operators have something to corrupt in every section of the body, not just the simplest
/// possible shape.
fn richer_ir() -> Ir {
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
                message: Some(crate::model::Text::utf8("initial")),
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
    let child_blob = b
        .add_blob(b"second revision, a bit longer than the first".to_vec())
        .unwrap();
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

/// The two-atom fixture `artifact::tests` also builds, kept independent of it (that module's `build` is
/// private to it) so this module has a second, simpler seed shape.
fn simple_ir() -> Ir {
    let mut b = IrBuilder::new(provenance());
    let blob = b.add_blob(b"x".to_vec()).unwrap();
    b.add_atom(AtomDraft {
        parents: vec![],
        ops: vec![PathOp::Add {
            path: "a".into(),
            blob,
            mode: 0o100_644,
            status: EpistemicStatus::Stated,
        }],
        copies: vec![],
        metadata: MetadataClaims::default(),
        source: SourceIdentity {
            kind: SourceKind::Git,
            repo_id: b"repo".to_vec(),
            atom_id: b"c0".to_vec(),
            signatures: Vec::new(),
            extras: Vec::new(),
        },
        status: EpistemicStatus::Stated,
    })
    .unwrap();
    b.finish().unwrap()
}

/// A decoded artifact must never claim more total blob content, atoms, refs, drops or flags than the
/// bytes it was decoded from could physically have held — the one module-specific "ceiling" this format
/// has, everything else being bounded by the reader's own length checks (`CanonReader::raw`/`list`,
/// checked against `remaining()` before any allocation).
fn check(input_len: usize, decoded: &super::Decoded) -> Outcome {
    let total_blob_bytes = usize::try_from(decoded.ir.content.total_bytes()).unwrap_or(usize::MAX);
    if total_blob_bytes > input_len {
        return Outcome::Violation(format!(
            "decoded {total_blob_bytes} bytes of blob content from a {input_len}-byte artifact"
        ));
    }
    let record_count = decoded.ir.atoms.len()
        + decoded.ir.refs.len()
        + decoded.ir.loss.dropped.len()
        + decoded.ir.flags.len();
    if record_count > input_len {
        return Outcome::Violation(format!(
            "decoded {record_count} records from a {input_len}-byte artifact"
        ));
    }
    Outcome::Ok
}

fn check_raw(bytes: &[u8]) -> Outcome {
    match from_bytes(bytes) {
        Ok(decoded) => check(bytes.len(), &decoded),
        Err(_) => Outcome::Ok,
    }
}

/// Split `to_bytes(ir)`'s output into `(prefix, body)`, where `prefix` is everything the digest is
/// computed over *together with* `body` (i.e. `version_bytes`), and `body` is everything after the
/// stored digest — mirroring `from_bytes`'s own layout exactly (RFC 011 §2.3): `magic(8) · format(1) ·
/// version · digest(32) · body`.
fn split_header(bytes: &[u8]) -> (Vec<u8>, Vec<u8>) {
    // magic(8) + format(1) are fixed-width and never part of the digest; version is a run of three
    // uvarints, so its length must be read the same way `from_bytes` reads it.
    let after_format = bytes
        .get(9..)
        .expect("artifact long enough for format byte");
    let mut r = crate::canon::CanonReader::new(after_format);
    let _major = r.uvarint().unwrap();
    let _minor = r.uvarint().unwrap();
    let _patch = r.uvarint().unwrap();
    let version_len = after_format.len() - r.remaining();
    let version_bytes = after_format.get(..version_len).unwrap().to_vec();
    let fixed_prefix = bytes.get(..9).unwrap().to_vec();
    let digest_start = 9 + version_len;
    let body = bytes.get(digest_start + 32..).unwrap().to_vec();
    let mut prefix = fixed_prefix;
    prefix.extend_from_slice(&version_bytes);
    (prefix, body)
}

/// Reassemble `(prefix, mutated_body)` into a full artifact with a **correct** digest over
/// `prefix[9..] (version) ++ mutated_body` — a mutation that is wrong only in the body, correctly signed,
/// exactly what `from_bytes`'s structural checks exist to catch.
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

fn check_resigned(prefix: &[u8], mutated_body: &[u8]) -> Outcome {
    let full = resign(prefix, mutated_body);
    match from_bytes(&full) {
        Ok(decoded) => check(full.len(), &decoded),
        Err(_) => Outcome::Ok,
    }
}

/// The body's own leading `uvarint(meta_len)` is the one position every seed can locate without
/// re-implementing the rest of the encoder: it is always the very first field of the body.
fn meta_len_position(body: &[u8]) -> InterestingPosition {
    let mut r = crate::canon::CanonReader::new(body);
    let before = r.remaining();
    let _ = r.uvarint();
    let len = before - r.remaining();
    InterestingPosition {
        offset: 0,
        encoding: Encoding::Uvarint { len },
        // The reader refuses a declared length past what remains (`uvarint_len`); the body's own
        // remaining length past this field is exactly that ceiling.
        ceiling: Some((body.len() - len) as u64),
    }
}

#[test]
fn from_bytes_on_mutated_raw_bytes_never_panics_and_never_overclaims() {
    let seeds: Vec<Vec<u8>> = vec![to_bytes(&richer_ir()), to_bytes(&simple_ir())];
    let specs: Vec<Seed<'_>> = seeds.iter().map(|b| Seed::generic(b)).collect();
    harness::run(
        env!("CARGO_MANIFEST_DIR"),
        "from_bytes_on_mutated_raw_bytes_never_panics_and_never_overclaims",
        &specs,
        QUICK_ITERATIONS,
        DEEP_FACTOR,
        TIME_BUDGET,
        |_seed_index, bytes| check_raw(bytes),
    );
}

#[test]
fn from_bytes_on_a_correctly_resigned_mutated_body_never_panics_and_never_overclaims() {
    let full_a = to_bytes(&richer_ir());
    let full_b = to_bytes(&simple_ir());
    let (prefix_a, body_a) = split_header(&full_a);
    let (prefix_b, body_b) = split_header(&full_b);
    let pos_a = meta_len_position(&body_a);
    let pos_b = meta_len_position(&body_b);
    let positions_a = [pos_a];
    let positions_b = [pos_b];
    let specs = [
        Seed::new(&body_a, &positions_a),
        Seed::new(&body_b, &positions_b),
    ];
    let prefixes = [prefix_a, prefix_b];
    harness::run(
        env!("CARGO_MANIFEST_DIR"),
        "from_bytes_on_a_correctly_resigned_mutated_body_never_panics_and_never_overclaims",
        &specs,
        QUICK_ITERATIONS,
        DEEP_FACTOR,
        TIME_BUDGET,
        |seed_index, mutated_body| {
            let prefix = prefixes
                .get(seed_index)
                .expect("the harness never passes an out-of-range seed index");
            check_resigned(prefix, mutated_body)
        },
    );
}
