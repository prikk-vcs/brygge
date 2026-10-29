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
//!
//! Both strategies additionally require [`read_from`] (the streaming reader, RFC 015 D-4, run here over
//! a fast in-memory `Cursor` rather than a real file) to agree with [`from_bytes`] on every mutated
//! input — the same `Ok`, or an `Err` of the same variant ([`check_equivalent`]) — so the two readers'
//! digest/structure precedence can never quietly diverge.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::time::Duration;

use brygge_mutate::harness::{self, Outcome, Seed};
use brygge_mutate::ops::{Encoding, InterestingPosition};
use sha2::{Digest, Sha256};

use super::{from_bytes, read_from, to_bytes};
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
/// bytes it was decoded from could physically have produced — this format's one module-specific
/// "ceiling", everything else being bounded by the reader's own length checks (`CanonReader::raw`/
/// `list`, checked against `remaining()` before any allocation).
///
/// **Redefined for contract 0.3.0** (RFC 018 §6.6): a frame's declared `raw_len` may legitimately exceed
/// the artifact's own byte size — that is what compression is for — so "never overclaims" is no longer
/// "never more than `input_len`". It is instead the bound [`frame::inflate_exact`] itself already
/// enforces per frame, before any allocation: `raw_len ≤ 1032 × stored_len + 64`. Summed over every
/// frame, `stored_len`'s total can be at most `input_len` and there can be at most `input_len` frames
/// (each needs at least one header byte), so `1096 × input_len` (`1032 + 64`) is a sound, if generous,
/// ceiling on *any* content a correctly-bounds-checked read of this artifact could produce — content or
/// record counts included, since the metadata is framed exactly the same way.
fn check(input_len: usize, decoded: &super::Decoded) -> Outcome {
    let ceiling = 1096u64.saturating_mul(input_len as u64);
    let total_blob_bytes = decoded.ir.content.total_bytes();
    if total_blob_bytes > ceiling {
        return Outcome::Violation(format!(
            "decoded {total_blob_bytes} bytes of blob content from a {input_len}-byte artifact \
             (ceiling {ceiling})"
        ));
    }
    let record_count = (decoded.ir.atoms.len()
        + decoded.ir.refs.len()
        + decoded.ir.loss.dropped.len()
        + decoded.ir.flags.len()) as u64;
    if record_count > ceiling {
        return Outcome::Violation(format!(
            "decoded {record_count} records from a {input_len}-byte artifact (ceiling {ceiling})"
        ));
    }
    Outcome::Ok
}

/// A bare label for an `Err`'s variant (RFC 015 D-4's equivalence check, below): comparable without
/// requiring `Decoded`/`Error` to implement `PartialEq` themselves, and exhaustive on purpose —
/// `Error` is `#[non_exhaustive]` only to crates *outside* this one, so a new variant here is a compile
/// error in this match, not a silently-ignored wildcard arm.
fn outcome_tag(r: &Result<super::Decoded, crate::Error>) -> &'static str {
    match r {
        Ok(_) => "ok",
        Err(crate::Error::Decode(_)) => "decode",
        Err(crate::Error::DigestMismatch) => "digest_mismatch",
        Err(crate::Error::PreReleaseArtifact) => "pre_release",
        Err(crate::Error::UnsupportedContract { .. }) => "unsupported_contract",
        Err(crate::Error::UnknownCriticalField { .. }) => "unknown_critical_field",
        Err(crate::Error::NonCanonical(_)) => "non_canonical",
        Err(crate::Error::Invariant(_)) => "invariant",
        Err(crate::Error::Io(_)) => "io",
        Err(crate::Error::ArtifactChanged(_)) => "artifact_changed",
    }
}

/// RFC 015 D-4's own equivalence requirement: on the exact same bytes, [`from_bytes`] and
/// [`read_from`] (over a `Cursor`, so this stays fast) must agree — the same `Ok`, or an `Err` of the
/// same variant. A `Cursor<Vec<u8>>` behind `read_from` exercises the streaming reader's own digest/
/// structure precedence on every seed the harness already generates, with no extra file I/O.
fn check_equivalent(bytes: &[u8]) -> Result<super::Decoded, Outcome> {
    let a = from_bytes(bytes);
    let b = read_from(std::io::Cursor::new(bytes.to_vec()));
    if outcome_tag(&a) != outcome_tag(&b) {
        return Err(Outcome::Violation(format!(
            "from_bytes and read_from disagree on a {}-byte input: {} vs {}",
            bytes.len(),
            outcome_tag(&a),
            outcome_tag(&b)
        )));
    }
    a.map_err(|_| Outcome::Ok)
}

fn check_raw(bytes: &[u8]) -> Outcome {
    match check_equivalent(bytes) {
        Ok(decoded) => check(bytes.len(), &decoded),
        Err(outcome) => outcome,
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
    match check_equivalent(&full) {
        Ok(decoded) => check(full.len(), &decoded),
        Err(outcome) => outcome,
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

// ---- RFC 018 §6.6: two more strategies, now that a frame sits between the bytes and the digest ------

/// A seed IR with one large, highly repetitive blob, so `write_to` is certain to choose
/// `Codec::Deflate` for its own frame (`frame::compress_frame` only does when that is strictly
/// smaller) — the "frames" strategy below needs a real DEFLATE frame to mutate.
fn deflate_ir() -> Ir {
    let mut b = IrBuilder::new(provenance());
    let blob = b
        .add_blob(b"the quick brown fox jumps over the lazy dog, ".repeat(400))
        .unwrap();
    b.add_atom(AtomDraft {
        parents: vec![],
        ops: vec![PathOp::Add {
            path: "big".into(),
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

/// "resigned, on the logical stream" (RFC 018 §6.6): mutate only the metadata's own decompressed bytes
/// — the blob table and blob frames are copied **verbatim** from the real artifact, byte for byte — then
/// re-frame the mutated metadata as a fresh, `Codec::Stored` section (so the mutated bytes reach
/// `Ir::decode_metadata` unchanged by any codec) and recompute the digest over the new logical stream.
/// Reaches the canonical-form checks directly, the same target the 0.2.0-era "resigned" strategy had,
/// now with the frame codec never standing in the way.
fn resign_v3_metadata_only(full: &[u8], mutated_meta_bytes: &[u8]) -> Vec<u8> {
    let (prefix, rest) = split_header(full);
    let version_bytes = prefix.get(9..).unwrap_or(&[]).to_vec();
    let mut r = crate::canon::CanonReader::new(&rest);

    // Skip past the original metadata frames (their bytes are being replaced), to reach the verbatim
    // table + blob-frame tail.
    let nframes_meta = r.uvarint().unwrap();
    for _ in 0..nframes_meta {
        let codec = crate::artifact::frame::Codec::from_u64(r.uvarint().unwrap()).unwrap();
        let _raw_len = r.uvarint().unwrap();
        let stored_len = r.uvarint_len().unwrap();
        let _ = codec;
        r.raw(stored_len).unwrap();
    }
    let tail_start = rest.len() - r.remaining();
    let tail = rest.get(tail_start..).unwrap().to_vec();

    // The tail's own hash contribution (table + blob content) is exactly its on-disk bytes for the
    // table part, and the inflated content for the blob-frame part — recomputed the same way
    // `from_bytes_v3` does, reusing its own frame reader so the two can never diverge.
    let mut tr = crate::canon::CanonReader::new(&tail);
    let nblobs = tr.uvarint().unwrap();
    let table_start = tail.len() - tr.remaining();
    let mut lens = Vec::with_capacity(nblobs as usize);
    for _ in 0..nblobs {
        tr.raw32().unwrap();
        lens.push(tr.uvarint().unwrap());
    }
    let table_end = tail.len() - tr.remaining();
    let table_bytes = tail.get(table_start..table_end).unwrap();

    let nframes_blob = tr.uvarint().unwrap();
    let mut blob_bytes = Vec::new();
    for _ in 0..nframes_blob {
        let codec = crate::artifact::frame::Codec::from_u64(tr.uvarint().unwrap()).unwrap();
        let raw_len = tr.uvarint().unwrap();
        let stored_len = tr.uvarint_len().unwrap();
        let stored = tr.raw(stored_len).unwrap();
        blob_bytes.extend_from_slice(
            &crate::artifact::frame::inflate_exact(codec, stored, raw_len).unwrap(),
        );
    }

    let mut hasher = Sha256::new();
    hasher.update(&version_bytes);
    hasher.update(crate::canon::uvarint_value(mutated_meta_bytes.len() as u64));
    hasher.update(mutated_meta_bytes);
    hasher.update(crate::canon::uvarint_value(nblobs));
    hasher.update(table_bytes);
    hasher.update(&blob_bytes);
    let digest: [u8; 32] = hasher.finalize().into();

    let mut out = prefix.get(..9).unwrap().to_vec();
    out.extend_from_slice(&version_bytes);
    out.extend_from_slice(&digest);
    let meta_len = mutated_meta_bytes.len() as u64;
    let chunk_lens = crate::artifact::frame::fixed_chunk_lens(meta_len);
    out.extend_from_slice(&crate::canon::uvarint_value(chunk_lens.len() as u64));
    let mut offset = 0usize;
    for len in chunk_lens {
        let len = len as usize;
        let chunk = mutated_meta_bytes.get(offset..offset + len).unwrap();
        out.extend_from_slice(&crate::canon::uvarint_value(0)); // codec: stored
        out.extend_from_slice(&crate::canon::uvarint_value(len as u64));
        out.extend_from_slice(&crate::canon::uvarint_value(len as u64));
        out.extend_from_slice(chunk);
        offset += len;
    }
    out.extend_from_slice(&tail[table_start..]);
    out
}

/// The metadata bytes alone, extracted from a real artifact's own frames (so the seed the harness
/// mutates is exactly what `Ir::decode_metadata` will see).
fn extract_meta_bytes(full: &[u8]) -> Vec<u8> {
    let (_, rest) = split_header(full);
    let mut r = crate::canon::CanonReader::new(&rest);
    let nframes = r.uvarint().unwrap();
    let mut out = Vec::new();
    for _ in 0..nframes {
        let codec = crate::artifact::frame::Codec::from_u64(r.uvarint().unwrap()).unwrap();
        let raw_len = r.uvarint().unwrap();
        let stored_len = r.uvarint_len().unwrap();
        let stored = r.raw(stored_len).unwrap();
        out.extend_from_slice(
            &crate::artifact::frame::inflate_exact(codec, stored, raw_len).unwrap(),
        );
    }
    out
}

#[test]
fn from_bytes_on_a_resigned_mutated_logical_metadata_never_panics_and_never_overclaims() {
    let full_a = to_bytes(&richer_ir());
    let full_b = to_bytes(&simple_ir());
    let meta_a = extract_meta_bytes(&full_a);
    let meta_b = extract_meta_bytes(&full_b);
    let specs = [Seed::generic(&meta_a), Seed::generic(&meta_b)];
    let fulls = [full_a, full_b];
    harness::run(
        env!("CARGO_MANIFEST_DIR"),
        "from_bytes_on_a_resigned_mutated_logical_metadata_never_panics_and_never_overclaims",
        &specs,
        QUICK_ITERATIONS,
        DEEP_FACTOR,
        TIME_BUDGET,
        |seed_index, mutated_meta| {
            let full = fulls
                .get(seed_index)
                .expect("the harness never passes an out-of-range seed index");
            let resigned = resign_v3_metadata_only(full, mutated_meta);
            match check_equivalent(&resigned) {
                Ok(decoded) => check(resigned.len(), &decoded),
                Err(outcome) => outcome,
            }
        },
    );
}

/// "frames" (RFC 018 §6.6): a seed with a real `Codec::Deflate` blob frame, mutated generically across
/// its *whole* bytes with no re-signing — most mutations land in that frame's stored bytes (by far the
/// largest span in a small artifact) and so exercise the inflater's own bounds (an invalid DEFLATE
/// stream, an output that would over/under-run `raw_len`), on top of the version/magic/digest coverage
/// `from_bytes_on_mutated_raw_bytes_never_panics_and_never_overclaims` already gives every artifact.
#[test]
fn from_bytes_on_a_mutated_deflate_frame_never_panics_and_never_overclaims() {
    let seed = to_bytes(&deflate_ir());
    let specs = [Seed::generic(&seed)];
    harness::run(
        env!("CARGO_MANIFEST_DIR"),
        "from_bytes_on_a_mutated_deflate_frame_never_panics_and_never_overclaims",
        &specs,
        QUICK_ITERATIONS,
        DEEP_FACTOR,
        TIME_BUDGET,
        |_seed_index, bytes| check_raw(bytes),
    );
}
