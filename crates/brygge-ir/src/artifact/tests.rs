//! Tests for the artifact codec: round-trip, determinism, integrity, versioning, canonical structure,
//! and panic-freedom on malformed input (RFC 011 §2.3/§2.5/§7/§10).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;

use super::*;
use crate::builder::{AtomDraft, IrBuilder};
use crate::canon::CanonReader;
use crate::model::{
    ChangedPart, DropRecord, Flag, FlagKind, ImportProvenance, LossBoundary, LossClass,
    MetadataClaims, PathOp, RefKind, RefRecord, SourceIdentity, SourceKind, TransformRecord,
};
use crate::status::EpistemicStatus;

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

fn build() -> Ir {
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
    let child_blob = b.add_blob(b"second".to_vec()).unwrap();
    let _child = b
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
    b.finish().unwrap()
}

#[test]
fn roundtrips() {
    let ir = build();
    let bytes = to_bytes(&ir);
    let decoded = from_bytes(&bytes).unwrap();
    assert_eq!(decoded.ir, ir);
    assert_eq!(decoded.skipped_non_critical_fields, 0);
}

#[test]
fn serialization_is_byte_deterministic() {
    let ir = build();
    assert_eq!(to_bytes(&ir), to_bytes(&ir));
}

/// Several atoms, a ref, a drop, a flag, and an empty and a large blob — RFC 015 D-2's property test:
/// `write_to` and `to_bytes` must never diverge, on a richer shape than [`build`]'s two-atom case.
fn build_rich() -> Ir {
    let mut b = IrBuilder::new(provenance());
    let empty_blob = b.add_blob(Vec::new()).unwrap();
    let large_blob = b.add_blob(vec![0x5au8; 200_000]).unwrap();
    let root = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![
                PathOp::Add {
                    path: "empty".into(),
                    blob: empty_blob,
                    mode: 0o100_644,
                    status: EpistemicStatus::Stated,
                },
                PathOp::Add {
                    path: "large".into(),
                    blob: large_blob,
                    mode: 0o100_644,
                    status: EpistemicStatus::Stated,
                },
            ],
            copies: vec![],
            metadata: MetadataClaims {
                message: Some(crate::model::Text::utf8("root")),
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
    b.add_ref(RefRecord {
        name: "main".to_string(),
        kind: RefKind::Branch,
        target: root,
        status: EpistemicStatus::Stated,
        source: None,
        annotation: None,
    })
    .unwrap();
    b.set_loss(LossBoundary {
        dropped: vec![DropRecord {
            class: LossClass::Representation,
            what: "packfiles".to_string(),
            reason: "representation, not assertion".to_string(),
            step: None,
        }],
    });
    b.add_flag(Flag {
        kind: FlagKind::ConventionViolation,
        what: "layout".to_string(),
        count: 1,
        reason: "test fixture".to_string(),
    });
    b.finish().unwrap()
}

#[test]
fn write_to_equals_to_bytes_and_from_bytes_accepts_it() {
    for ir in [build(), build_rich()] {
        let via_to_bytes = to_bytes(&ir);

        let mut cursor = std::io::Cursor::new(Vec::new());
        write_to(&ir, &mut cursor).unwrap();
        let via_write_to = cursor.into_inner();

        assert_eq!(via_to_bytes, via_write_to);
        let decoded = from_bytes(&via_write_to).unwrap();
        assert_eq!(decoded.ir, ir);
    }
}

/// Locate the digest's own byte range, independent of everything after it (both contracts: the digest
/// always immediately follows the version bytes, which are always three single uvarints for a
/// `major`/`minor`/`patch` this small).
fn locate_digest(bytes: &[u8]) -> std::ops::Range<usize> {
    let mut vr = CanonReader::new(&bytes[9..]);
    vr.uvarint().unwrap();
    vr.uvarint().unwrap();
    vr.uvarint().unwrap();
    let version_len = (bytes.len() - 9) - vr.remaining();
    let digest_start = 9 + version_len;
    digest_start..digest_start + 32
}

/// RFC 017 D-2: [`Decoded::digest`] is the digest the reader actually checked, not a value read back
/// from a fixed offset (which would bypass the reader's own verification) — proved by comparing it
/// against the bytes stored at the artifact's own digest offset, located independently via
/// [`locate_digest`]. `from_bytes` and `read_from` must agree.
#[test]
fn decoded_digest_equals_the_digest_stored_at_the_artifacts_header() {
    let ir = build_rich();
    let bytes = to_bytes(&ir);
    let stored: [u8; 32] = bytes[locate_digest(&bytes)].try_into().unwrap();

    let via_from_bytes = from_bytes(&bytes).unwrap();
    assert_eq!(via_from_bytes.digest, stored);

    let via_read_from = read_from(std::io::Cursor::new(bytes)).unwrap();
    assert_eq!(via_read_from.digest, stored);
}

/// RFC 015 D-4: [`read_from`] over an in-memory `Cursor` must agree with [`from_bytes`] on every
/// artifact `write_to` can produce — the same bytes, the same `Ir`, whichever reader is used.
#[test]
fn read_from_a_cursor_agrees_with_from_bytes() {
    for ir in [build(), build_rich()] {
        let bytes = to_bytes(&ir);
        let decoded = read_from(std::io::Cursor::new(bytes.clone())).unwrap();
        assert_eq!(decoded.ir, ir);
        // And every blob reads back correctly through the artifact-backed store, not just the id set
        // equality above.
        for id in ir.content.ids() {
            assert_eq!(
                decoded.ir.content.get(id).unwrap(),
                ir.content.get(id).unwrap()
            );
        }
    }
}

fn scratch_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "brygge-ir-artifact-test-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// [`from_file`] (RFC 015 D-4) over a real file agrees with [`from_bytes`] too.
#[test]
fn from_file_agrees_with_from_bytes() {
    let dir = scratch_dir("from-file");
    let path = dir.join("artifact");
    let ir = build_rich();
    std::fs::write(&path, to_bytes(&ir)).unwrap();
    let decoded = from_file(&path).unwrap();
    assert_eq!(decoded.ir, ir);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A blob of 300 high-entropy bytes (chained SHA-256 of a fixed seed), deterministic but incompressible
/// enough that `write_to`'s DEFLATE-or-stored choice always lands on `Codec::Stored` (RFC 018 D-1: the
/// compressed form would be larger, header overhead included) — so its bytes sit verbatim in the
/// artifact, byte for byte, as the next test needs.
fn incompressible_blob() -> Vec<u8> {
    let mut out = Vec::new();
    let mut h: [u8; 32] = Sha256::digest(b"brygge-ir artifact test seed").into();
    while out.len() < 300 {
        h = Sha256::digest(h).into();
        out.extend_from_slice(&h);
    }
    out.truncate(300);
    out
}

/// RFC 015 D-4's own safety property: a blob modified in the artifact file *after* `from_file`'s
/// verifying pass is never handed to a caller as if nothing happened — the next read of that blob must
/// fail, not silently return the new bytes.
#[test]
fn a_blob_changed_in_the_file_after_the_pass_is_never_read_as_the_new_bytes() {
    let dir = scratch_dir("changed-during-read");
    let path = dir.join("artifact");
    let mut b = IrBuilder::new(provenance());
    let blob = b.add_blob(incompressible_blob()).unwrap();
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
            atom_id: b"c1".to_vec(),
            signatures: Vec::new(),
            extras: Vec::new(),
        },
        status: EpistemicStatus::Stated,
    })
    .unwrap();
    let ir = b.finish().unwrap();
    let bytes = to_bytes(&ir);
    std::fs::write(&path, &bytes).unwrap();

    let decoded = from_file(&path).unwrap();
    let id = *ir.content.ids().next().expect("one blob");
    let original = decoded.ir.content.get(&id).unwrap().unwrap();
    assert_eq!(original, incompressible_blob());

    // Flip one byte of that very blob's bytes in the file, in place (same length, so every other
    // blob's offset is undisturbed) — found the slow but foolproof way, by searching for the blob's own
    // bytes in the artifact, rather than assuming its offset.
    let pos = bytes
        .windows(original.len())
        .position(|w| w == original.as_slice())
        .expect("the blob's bytes are findable verbatim in the artifact");
    {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        f.seek(SeekFrom::Start(pos as u64 + 1)).unwrap();
        f.write_all(&[original[1] ^ 0xff]).unwrap();
    }

    let err = decoded.ir.content.get(&id).unwrap_err();
    assert!(matches!(err, Error::ArtifactChanged(_)), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn truncation_is_rejected_not_panicked() {
    let ir = build();
    let bytes = to_bytes(&ir);
    let cut = &bytes[..bytes.len() - 1];
    assert!(from_bytes(cut).is_err());
    assert!(from_bytes(b"BRYGGEIR").is_err());
    assert!(from_bytes(b"").is_err());
}

#[test]
fn bad_magic_is_rejected() {
    let mut bytes = to_bytes(&build());
    bytes[0] = b'X';
    assert!(matches!(from_bytes(&bytes), Err(Error::Decode(_))));
}

// ---- version gate (RFC 011 §2.3) -----------------------------------------------------------------

#[test]
fn format_1_is_a_pre_release_artifact_and_metadata_is_never_parsed() {
    // Garbage after the format byte: if metadata were ever touched, this would fail differently
    // (a Decode/NonCanonical error from the garbage), proving the read stops right after the format
    // byte.
    let mut bytes = MAGIC.to_vec();
    bytes.push(1);
    bytes.extend_from_slice(&[0xff; 64]);
    match from_bytes(&bytes) {
        Err(Error::PreReleaseArtifact) => {}
        other => panic!("expected PreReleaseArtifact, got {other:?}"),
    }
}

#[test]
fn format_1_with_no_trailing_bytes_at_all_is_still_a_pre_release_artifact() {
    let mut bytes = MAGIC.to_vec();
    bytes.push(1);
    match from_bytes(&bytes) {
        Err(Error::PreReleaseArtifact) => {}
        other => panic!("expected PreReleaseArtifact, got {other:?}"),
    }
}

fn container_with_version(ir: &Ir, major: u64, minor: u64, patch: u64) -> Vec<u8> {
    let mut version = crate::canon::CanonWriter::new();
    version.uvarint(major);
    version.uvarint(minor);
    version.uvarint(patch);

    let mut body = crate::canon::CanonWriter::new();
    let meta = ir.encode_metadata();
    body.uvarint(meta.len() as u64);
    body.raw(&meta);
    body.uvarint(ir.content.len() as u64);
    for id in ir.content.ids() {
        let bytes = ir.content.get(id).unwrap().unwrap();
        body.raw(id.as_bytes());
        body.uvarint(bytes.len() as u64);
        body.raw(&bytes);
    }

    let mut hasher = Sha256::new();
    hasher.update(version.as_bytes());
    hasher.update(body.as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    let mut out = MAGIC.to_vec();
    out.push(FORMAT);
    out.extend_from_slice(version.as_bytes());
    out.extend_from_slice(&digest);
    out.extend_from_slice(body.as_bytes());
    out
}

#[test]
fn a_matching_patch_bump_is_accepted() {
    let ir = build();
    let bytes = container_with_version(&ir, 0, 2, 7);
    assert!(from_bytes(&bytes).is_ok());
}

#[test]
fn an_unlisted_minor_under_major_zero_is_refused() {
    let ir = build();
    let bytes = container_with_version(&ir, 0, u64::from(version::CURRENT.minor) + 1, 0);
    match from_bytes(&bytes) {
        Err(Error::UnsupportedContract { .. }) => {}
        other => panic!("expected UnsupportedContract, got {other:?}"),
    }
}

#[test]
fn a_newer_major_is_refused() {
    let ir = build();
    let bytes = container_with_version(&ir, 1, 0, 0);
    match from_bytes(&bytes) {
        Err(Error::UnsupportedContract { .. }) => {}
        other => panic!("expected UnsupportedContract, got {other:?}"),
    }
}

// ---- digest (RFC 011 §2.3/§7.5): one byte flipped in each section ---------------------------------

/// A contract-0.3.0 container for `ir`, with every frame forced to `Codec::Stored` (bypassing
/// `write_to`'s own DEFLATE-or-stored choice): so a byte flip anywhere in a frame's stored bytes is a
/// flip of the *logical* bytes too, one-for-one, deterministically reaching the digest check rather
/// than risking an "a frame that does not inflate" refusal from a DEFLATE stream a single flipped bit
/// happens to corrupt structurally. Returns the bytes alongside `(digest_start, meta_stored_start,
/// blob_stored_start)`.
fn container_v3_all_stored(ir: &Ir) -> (Vec<u8>, usize, usize, usize) {
    let mut version = CanonWriter::new();
    version.uvarint(0);
    version.uvarint(3);
    version.uvarint(0);

    let meta_bytes = ir.encode_metadata();
    let order = store_order(ir);
    let mut hasher = Sha256::new();
    hasher.update(version.as_bytes());
    hasher.update(canon::uvarint_value(meta_bytes.len() as u64));
    hasher.update(&meta_bytes);
    hasher.update(canon::uvarint_value(order.len() as u64));
    let mut blob_bytes: Vec<Vec<u8>> = Vec::new();
    for id in &order {
        hasher.update(id.as_bytes());
        let bytes = ir.content.get(id).unwrap().unwrap();
        hasher.update(canon::uvarint_value(bytes.len() as u64));
        blob_bytes.push(bytes);
    }
    for bytes in &blob_bytes {
        hasher.update(bytes);
    }
    let digest: [u8; 32] = hasher.finalize().into();

    let mut out = MAGIC.to_vec();
    out.push(FORMAT);
    out.extend_from_slice(version.as_bytes());
    let digest_start = out.len();
    out.extend_from_slice(&digest);

    // One stored metadata frame.
    out.extend_from_slice(&canon::uvarint_value(1));
    out.extend_from_slice(&canon::uvarint_value(0)); // codec: stored
    out.extend_from_slice(&canon::uvarint_value(meta_bytes.len() as u64));
    out.extend_from_slice(&canon::uvarint_value(meta_bytes.len() as u64));
    let meta_stored_start = out.len();
    out.extend_from_slice(&meta_bytes);

    out.extend_from_slice(&canon::uvarint_value(order.len() as u64));
    for (id, bytes) in order.iter().zip(blob_bytes.iter()) {
        out.extend_from_slice(id.as_bytes());
        out.extend_from_slice(&canon::uvarint_value(bytes.len() as u64));
    }
    // One stored frame per blob (simplest valid packing when every blob is tiny: each still obeys
    // `canonical_frame_raw_lens`'s rule only if blobs are grouped per that rule — `build()`'s two
    // blobs are small enough that the real writer would pack them into one frame, so match that here
    // too, to stay a canonical, acceptable artifact for the version/blob-byte tests, which don't care
    // about packing specifics, only about where the digest lands.
    let total: u64 = blob_bytes.iter().map(|b| b.len() as u64).sum();
    out.extend_from_slice(&canon::uvarint_value(1));
    out.extend_from_slice(&canon::uvarint_value(0)); // codec: stored
    out.extend_from_slice(&canon::uvarint_value(total));
    out.extend_from_slice(&canon::uvarint_value(total));
    let blob_stored_start = out.len();
    for bytes in &blob_bytes {
        out.extend_from_slice(bytes);
    }

    (out, digest_start, meta_stored_start, blob_stored_start)
}

#[test]
fn a_flipped_version_byte_is_detected_as_a_digest_mismatch() {
    let ir = build();
    let (mut bytes, digest_start, ..) = container_v3_all_stored(&ir);
    // Flip the low bit of the patch byte: stays a valid minimal single-byte uvarint, and patch is
    // ignored by the version gate (RFC 011 D-3), so this reaches the digest check.
    bytes[digest_start - 1] ^= 0x01;
    match from_bytes(&bytes) {
        Err(Error::DigestMismatch) => {}
        other => panic!("expected DigestMismatch, got {other:?}"),
    }
}

#[test]
fn a_flipped_metadata_byte_is_detected_as_a_digest_mismatch() {
    let ir = build();
    let (mut bytes, _, meta_stored_start, blob_stored_start) = container_v3_all_stored(&ir);
    assert!(blob_stored_start > meta_stored_start);
    bytes[meta_stored_start] ^= 0xff;
    match from_bytes(&bytes) {
        Err(Error::DigestMismatch) => {}
        other => panic!("expected DigestMismatch, got {other:?}"),
    }
}

#[test]
fn a_flipped_blob_store_byte_is_detected_as_a_digest_mismatch() {
    let ir = build();
    let (mut bytes, .., blob_stored_start) = container_v3_all_stored(&ir);
    assert!(bytes.len() > blob_stored_start);
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    match from_bytes(&bytes) {
        Err(Error::DigestMismatch) => {}
        other => panic!("expected DigestMismatch, got {other:?}"),
    }
}

// ---- structural checks (RFC 011 §2.5) -------------------------------------------------------------

#[test]
fn atoms_out_of_canonical_order_are_rejected() {
    let mut ir = build();
    ir.atoms.reverse(); // child before its own parent — breaks both order and "earlier atom" rules
    let bytes = to_bytes(&ir);
    match from_bytes(&bytes) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn a_copy_naming_a_later_atom_as_from_atom_is_rejected() {
    // Two independent roots (no parent relationship, so their relative order is a pure tiebreak and
    // stays canonical either way): X's copy names Y — the atom stored *after* it — as its from_atom.
    // A from_atom must be strictly earlier, never later, independent of the parent/topo-order rule.
    let mut ir = build();
    let mut y = ir.atoms[1].clone();
    y.parents.clear(); // make Y a second independent root, unrelated to X
    y.copies.clear();
    y.id = y.compute_id();
    let mut x = ir.atoms[0].clone();
    x.copies.push(crate::model::CopyRecord {
        from: "readme".into(),
        from_atom: y.id,
        to: "impossible".into(),
        status: EpistemicStatus::Stated,
    });
    x.id = x.compute_id(); // keep the stored id honest after mutating content
    ir.atoms = vec![x, y];
    let bytes = to_bytes(&ir);
    match from_bytes(&bytes) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn a_duplicate_atom_id_is_rejected() {
    let mut ir = build();
    let dup = ir.atoms[0].clone();
    ir.atoms.push(dup);
    let bytes = to_bytes(&ir);
    match from_bytes(&bytes) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn an_orphan_blob_is_rejected_by_the_writer() {
    // RFC 018 D-2: `store_order` only names a blob some op references, so `write_to` now catches an
    // orphan itself (fail fast) rather than silently excluding it from the artifact.
    let mut ir = build();
    ir.content.insert(b"nobody references me".to_vec()).unwrap();
    let mut cursor = std::io::Cursor::new(Vec::new());
    match write_to(&ir, &mut cursor) {
        Err(Error::Invariant(m)) => assert!(m.contains("orphan"), "{m}"),
        other => panic!("expected Invariant (orphan), got {other:?}"),
    }
}

/// The read-side counterpart (RFC 011 §2.5): an externally-crafted 0.3.0 artifact whose blob table
/// names a blob no op references is still rejected on read, independent of `write_to`'s own guard.
#[test]
fn an_orphan_blob_in_a_hand_built_artifact_is_rejected_on_read() {
    let ir = build();
    let extra = b"nobody references me".to_vec();
    let extra_id = BlobId::of(&extra);
    let mut order = store_order(&ir);
    // The orphan sorts last: its "path" (none) is not a real op, so it is simply appended after the
    // real table, in id order relative to nothing — any position is fine, since the orphan check runs
    // after the id-set reconciliation below, not the order check.
    order.push(extra_id);
    let mut lens: Vec<u64> = Vec::new();
    for id in &order {
        lens.push(if *id == extra_id {
            extra.len() as u64
        } else {
            ir.content.get(id).unwrap().unwrap().len() as u64
        });
    }

    let mut version = crate::canon::CanonWriter::new();
    version.uvarint(0);
    version.uvarint(3);
    version.uvarint(0);

    let meta_bytes = ir.encode_metadata();
    let mut hasher = Sha256::new();
    hasher.update(version.as_bytes());
    hasher.update(canon::uvarint_value(meta_bytes.len() as u64));
    hasher.update(&meta_bytes);
    hasher.update(canon::uvarint_value(order.len() as u64));
    for (id, len) in order.iter().zip(lens.iter()) {
        hasher.update(id.as_bytes());
        hasher.update(canon::uvarint_value(*len));
    }
    for id in &order {
        let bytes = if *id == extra_id {
            extra.clone()
        } else {
            ir.content.get(id).unwrap().unwrap()
        };
        hasher.update(&bytes);
    }
    let digest: [u8; 32] = hasher.finalize().into();

    let mut out = MAGIC.to_vec();
    out.push(FORMAT);
    out.extend_from_slice(version.as_bytes());
    out.extend_from_slice(&digest);
    write_frames(
        &mut out,
        &meta_bytes,
        &frame::fixed_chunk_lens(meta_bytes.len() as u64),
        false,
    )
    .unwrap();
    out.extend_from_slice(&canon::uvarint_value(order.len() as u64));
    for (id, len) in order.iter().zip(lens.iter()) {
        out.extend_from_slice(id.as_bytes());
        out.extend_from_slice(&canon::uvarint_value(*len));
    }
    let frame_lens = frame::canonical_frame_raw_lens(lens.iter().copied());
    out.extend_from_slice(&canon::uvarint_value(frame_lens.len() as u64));
    let mut blob_iter = order.iter();
    for &flen in &frame_lens {
        let mut buf = Vec::new();
        let mut collected = 0u64;
        while collected < flen {
            let id = blob_iter.next().unwrap();
            let bytes = if *id == extra_id {
                extra.clone()
            } else {
                ir.content.get(id).unwrap().unwrap()
            };
            collected += bytes.len() as u64;
            buf.extend_from_slice(&bytes);
        }
        write_one_frame(&mut out, &buf, false).unwrap();
    }

    // Caught as the table no longer matching `store_order` (an orphan is, definitionally, a blob no op
    // places in that order) — the same NonCanonical outcome the pre-RFC-018 reader gave directly from
    // the orphan check; here it is reached one step earlier, by the order check that now stands in
    // front of it.
    match from_bytes(&out) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn a_ref_targeting_an_unknown_atom_is_rejected() {
    let mut ir = build();
    ir.refs.push(crate::model::RefRecord {
        name: "refs/heads/ghost".into(),
        kind: crate::model::RefKind::Branch,
        target: crate::model::AtomId([0xEE; 32]),
        status: EpistemicStatus::Stated,
        source: None,
        annotation: None,
    });
    let bytes = to_bytes(&ir);
    match from_bytes(&bytes) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

// ---- unknown fields (RFC 011 §2.2 rule 9) ---------------------------------------------------------

/// Append a trailing field (an id greater than every real field id, so ascending order still holds) to
/// an already-encoded record's bytes.
fn inject_trailing_field(record_bytes: &[u8], id: u64, critical: bool, value: &[u8]) -> Vec<u8> {
    let mut r = CanonReader::new(record_bytes);
    let n = r.uvarint().unwrap();
    let consumed = record_bytes.len() - r.remaining();
    let rest = &record_bytes[consumed..];
    let mut w = crate::canon::CanonWriter::new();
    w.uvarint(n + 1);
    w.raw(rest);
    w.uvarint((id << 1) | u64::from(critical));
    w.uvarint(value.len() as u64);
    w.raw(value);
    w.into_bytes()
}

fn container_with_meta(ir: &Ir, meta: &[u8]) -> Vec<u8> {
    let mut version = crate::canon::CanonWriter::new();
    version.uvarint(0);
    version.uvarint(2);
    version.uvarint(0);

    let mut body = crate::canon::CanonWriter::new();
    body.uvarint(meta.len() as u64);
    body.raw(meta);
    body.uvarint(ir.content.len() as u64);
    for id in ir.content.ids() {
        let bytes = ir.content.get(id).unwrap().unwrap();
        body.raw(id.as_bytes());
        body.uvarint(bytes.len() as u64);
        body.raw(&bytes);
    }

    let mut hasher = Sha256::new();
    hasher.update(version.as_bytes());
    hasher.update(body.as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    let mut out = MAGIC.to_vec();
    out.push(FORMAT);
    out.extend_from_slice(version.as_bytes());
    out.extend_from_slice(&digest);
    out.extend_from_slice(body.as_bytes());
    out
}

#[test]
fn trailing_bytes_in_metadata_are_rejected() {
    let ir = build();
    let mut meta = ir.encode_metadata();
    meta.extend_from_slice(b"junk"); // the outer length prefix will cover this, so it lands *inside*
    // the metadata span, where the Ir record itself doesn't consume it.
    let bytes = container_with_meta(&ir, &meta);
    match from_bytes(&bytes) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn trailing_bytes_after_the_blob_store_are_rejected() {
    // A hand-built **contract 0.2.0** body (the prior, unframed layout — `to_bytes` no longer writes
    // this contract, but this build still reads it) with trailing junk appended to the stored bytes,
    // so the digest — which covers exactly those bytes — still matches, and the failure is
    // unambiguously the trailing-bytes check below it, not a digest mismatch.
    let ir = build();
    let mut version = crate::canon::CanonWriter::new();
    version.uvarint(0);
    version.uvarint(2);
    version.uvarint(0);

    let mut body = crate::canon::CanonWriter::new();
    let meta = ir.encode_metadata();
    body.uvarint(meta.len() as u64);
    body.raw(&meta);
    body.uvarint(ir.content.len() as u64);
    for id in ir.content.ids() {
        let bytes = ir.content.get(id).unwrap().unwrap();
        body.raw(id.as_bytes());
        body.uvarint(bytes.len() as u64);
        body.raw(&bytes);
    }
    let mut body = body.into_bytes();
    body.extend_from_slice(b"junk");

    let mut hasher = Sha256::new();
    hasher.update(version.as_bytes());
    hasher.update(&body);
    let digest: [u8; 32] = hasher.finalize().into();
    let mut out = MAGIC.to_vec();
    out.push(FORMAT);
    out.extend_from_slice(version.as_bytes());
    out.extend_from_slice(&digest);
    out.extend_from_slice(&body);
    match from_bytes(&out) {
        Err(Error::NonCanonical(_)) => {}
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

#[test]
fn an_unknown_non_critical_field_on_ir_is_skipped_and_counted() {
    let ir = build();
    let meta = inject_trailing_field(&ir.encode_metadata(), 99, false, b"from the future");
    let bytes = container_with_meta(&ir, &meta);
    let decoded = from_bytes(&bytes).unwrap();
    assert_eq!(decoded.skipped_non_critical_fields, 1);
    assert_eq!(decoded.ir, ir);
}

#[test]
fn an_unknown_critical_field_on_ir_is_rejected() {
    let ir = build();
    let meta = inject_trailing_field(&ir.encode_metadata(), 99, true, b"from the future");
    let bytes = container_with_meta(&ir, &meta);
    match from_bytes(&bytes) {
        Err(Error::UnknownCriticalField {
            record: "Ir",
            tag: 99,
        }) => {}
        other => panic!("expected UnknownCriticalField, got {other:?}"),
    }
}

// ---- panic-freedom (RFC 011 §10, brygge-03) -------------------------------------------------------

#[test]
fn truncations_and_single_byte_mutations_never_panic() {
    let ir = build();
    let bytes = to_bytes(&ir);

    for n in 0..=bytes.len() {
        let _ = from_bytes(&bytes[..n]); // must return Ok or Err, never panic
    }
    for i in 0..bytes.len() {
        let mut mutated = bytes.clone();
        mutated[i] ^= 0xff;
        let _ = from_bytes(&mutated); // must return Ok or Err, never panic
    }
}

// ---- canonical order of refs, drops and flags is on the variant numbers (batch I) ------------------

const ALL_REF_KINDS: [fn() -> RefKind; 5] = [
    || RefKind::Branch,
    || RefKind::Tag,
    || RefKind::Bookmark,
    || RefKind::NamedBranch,
    || RefKind::Other("phase".to_string()),
];
const ALL_LOSS_CLASSES: [LossClass; 3] = [
    LossClass::Representation,
    LossClass::AdvisoryUnreliable,
    LossClass::Other,
];
const ALL_FLAG_KINDS: [FlagKind; 2] = [
    FlagKind::ConventionViolation,
    FlagKind::BelowConfidenceFloor,
];

fn ref_of(ir: &Ir, name: &str, kind: RefKind) -> RefRecord {
    RefRecord {
        name: name.into(),
        kind,
        target: ir.atoms[0].id,
        status: EpistemicStatus::Stated,
        source: None,
        annotation: None,
    }
}

fn drop_of(class: LossClass, what: &str) -> DropRecord {
    DropRecord {
        class,
        what: what.into(),
        reason: "r".into(),
        step: None,
    }
}

fn flag_of(kind: FlagKind, what: &str) -> Flag {
    Flag {
        kind,
        what: what.into(),
        count: 1,
        reason: "r".into(),
    }
}

/// A builder over the shared two-atom fixture, so the atoms and blobs are the same as `build()`'s.
fn builder_with_atoms() -> IrBuilder {
    let ir = build();
    let mut b = IrBuilder::new(ir.provenance.clone());
    for atom in &ir.atoms {
        for op in &atom.ops {
            if let PathOp::Add { blob, .. } = op {
                b.add_blob(ir.content.get(blob).unwrap().unwrap()).unwrap();
            }
        }
        b.add_atom(AtomDraft {
            parents: atom.parents.clone(),
            ops: atom.ops.clone(),
            copies: atom.copies.clone(),
            metadata: atom.metadata.clone(),
            source: atom.source.clone(),
            status: atom.status.clone(),
        })
        .unwrap();
    }
    b
}

#[test]
fn every_pair_of_ref_kinds_roundtrips_in_both_insertion_orders() {
    let target = build().atoms[0].id;
    for a in ALL_REF_KINDS {
        for b in ALL_REF_KINDS {
            if a() == b() {
                continue;
            }
            let mut builder = builder_with_atoms();
            for kind in [a(), b()] {
                builder
                    .add_ref(RefRecord {
                        name: "x".into(),
                        kind,
                        target,
                        status: EpistemicStatus::Stated,
                        source: None,
                        annotation: None,
                    })
                    .unwrap();
            }
            let ir = builder.finish().unwrap();
            let read = from_bytes(&to_bytes(&ir)).unwrap_or_else(|e| panic!("{a:?}/{b:?}: {e}"));
            assert_eq!(read.ir.refs, ir.refs);
            let variants: Vec<u8> = ir.refs.iter().map(|r| r.kind.variant()).collect();
            assert!(variants.windows(2).all(|w| w[0] < w[1]), "{variants:?}");
        }
    }
}

#[test]
fn every_pair_of_loss_classes_roundtrips_in_both_insertion_orders() {
    for a in ALL_LOSS_CLASSES {
        for b in ALL_LOSS_CLASSES {
            if a == b {
                continue;
            }
            let mut builder = builder_with_atoms();
            builder.set_loss(LossBoundary {
                dropped: vec![drop_of(a, "same"), drop_of(b, "same")],
            });
            let ir = builder.finish().unwrap();
            let read = from_bytes(&to_bytes(&ir)).unwrap_or_else(|e| panic!("{a:?}/{b:?}: {e}"));
            assert_eq!(read.ir.loss, ir.loss);
            assert!(ir.loss.dropped[0].class.variant() < ir.loss.dropped[1].class.variant());
        }
    }
}

#[test]
fn every_pair_of_flag_kinds_roundtrips_in_both_insertion_orders() {
    for a in ALL_FLAG_KINDS {
        for b in ALL_FLAG_KINDS {
            if a == b {
                continue;
            }
            let mut builder = builder_with_atoms();
            builder.add_flag(flag_of(a, "same"));
            builder.add_flag(flag_of(b, "same"));
            let ir = builder.finish().unwrap();
            let read = from_bytes(&to_bytes(&ir)).unwrap_or_else(|e| panic!("{a:?}/{b:?}: {e}"));
            assert_eq!(read.ir.flags, ir.flags);
            assert!(ir.flags[0].kind.variant() < ir.flags[1].kind.variant());
        }
    }
}

/// The three keys are `(name, variant)`, `(class variant, what)` and `(kind variant, what)`: within one
/// variant the name (or `what`) decides, and a different name sorts by bytes, not by the variant.
#[test]
fn names_order_before_variants_for_refs_and_variants_before_whats_for_drops_and_flags() {
    let mut builder = builder_with_atoms();
    let target = build().atoms[0].id;
    for (name, kind) in [
        ("b", RefKind::Branch),
        ("a", RefKind::NamedBranch),
        ("a", RefKind::Tag),
    ] {
        builder
            .add_ref(RefRecord {
                name: name.into(),
                kind,
                target,
                status: EpistemicStatus::Stated,
                source: None,
                annotation: None,
            })
            .unwrap();
    }
    builder.set_loss(LossBoundary {
        dropped: vec![
            drop_of(LossClass::Other, "a"),
            drop_of(LossClass::Representation, "z"),
        ],
    });
    builder.add_flag(flag_of(FlagKind::BelowConfidenceFloor, "a"));
    builder.add_flag(flag_of(FlagKind::ConventionViolation, "z"));
    let ir = builder.finish().unwrap();
    from_bytes(&to_bytes(&ir)).unwrap();
    let refs: Vec<_> = ir
        .refs
        .iter()
        .map(|r| (r.name.as_str(), r.kind.variant()))
        .collect();
    assert_eq!(refs, [("a", 1), ("a", 3), ("b", 0)]);
    assert_eq!(ir.loss.dropped[0].what, "z");
    assert_eq!(ir.flags[0].what, "z");
}

fn assert_non_canonical(ir: &Ir, what: &str) {
    match from_bytes(&to_bytes(ir)) {
        Err(Error::NonCanonical(m)) => assert!(m.contains(what), "{m}"),
        other => panic!("expected NonCanonical ({what}), got {other:?}"),
    }
}

#[test]
fn a_hand_built_stream_with_a_pair_in_the_wrong_variant_order_is_non_canonical() {
    for a in ALL_REF_KINDS {
        for b in ALL_REF_KINDS {
            if a().variant() <= b().variant() {
                continue;
            }
            let mut ir = build();
            ir.refs = vec![ref_of(&ir, "x", a()), ref_of(&ir, "x", b())];
            assert_non_canonical(&ir, "Ir.refs");
        }
    }
    for a in ALL_LOSS_CLASSES {
        for b in ALL_LOSS_CLASSES {
            if a.variant() <= b.variant() {
                continue;
            }
            let mut ir = build();
            ir.loss.dropped = vec![drop_of(a, "same"), drop_of(b, "same")];
            assert_non_canonical(&ir, "Ir.dropped");
        }
    }
    for a in ALL_FLAG_KINDS {
        for b in ALL_FLAG_KINDS {
            if a.variant() <= b.variant() {
                continue;
            }
            let mut ir = build();
            ir.flags = vec![flag_of(a, "same"), flag_of(b, "same")];
            assert_non_canonical(&ir, "Ir.flags");
        }
    }
}

#[test]
fn a_hand_built_stream_with_a_duplicate_key_is_non_canonical() {
    let mut ir = build();
    ir.refs = vec![
        ref_of(&ir, "x", RefKind::Other("one".into())),
        ref_of(&ir, "x", RefKind::Other("two".into())),
    ];
    assert_non_canonical(&ir, "Ir.refs");
    let mut ir = build();
    ir.loss.dropped = vec![
        drop_of(LossClass::Other, "w"),
        drop_of(LossClass::Other, "w"),
    ];
    assert_non_canonical(&ir, "Ir.dropped");
    let mut ir = build();
    ir.flags = vec![
        flag_of(FlagKind::ConventionViolation, "w"),
        flag_of(FlagKind::ConventionViolation, "w"),
    ];
    assert_non_canonical(&ir, "Ir.flags");
}

#[test]
fn the_builder_refuses_a_duplicate_key_instead_of_writing_an_unreadable_artifact() {
    let target = build().atoms[0].id;
    let mut b = builder_with_atoms();
    for label in ["one", "two"] {
        b.add_ref(RefRecord {
            name: "x".into(),
            kind: RefKind::Other(label.into()),
            target,
            status: EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    }
    assert!(matches!(b.finish(), Err(Error::Invariant(_))));

    let mut b = builder_with_atoms();
    b.set_loss(LossBoundary {
        dropped: vec![
            drop_of(LossClass::Other, "w"),
            drop_of(LossClass::Other, "w"),
        ],
    });
    assert!(matches!(b.finish(), Err(Error::Invariant(_))));

    let mut b = builder_with_atoms();
    b.add_flag(flag_of(FlagKind::BelowConfidenceFloor, "w"));
    b.add_flag(flag_of(FlagKind::BelowConfidenceFloor, "w"));
    assert!(matches!(b.finish(), Err(Error::Invariant(_))));
}

// ---- RFC 018: compact artifacts (contract 0.3.0) --------------------------------------------------

/// The shape of `artifact::mutation::richer_ir`, built independently here (that module is test-only
/// and not reused across these files on purpose) for the golden-bytes and 0.2.0-fixture proofs: two
/// atoms, a ref, a drop, a flag, and two blobs, one empty.
fn fixture_ir() -> Ir {
    let mut b = IrBuilder::new(provenance());
    let empty_blob = b.add_blob(Vec::new()).unwrap();
    let blob = b.add_blob(b"hello world".to_vec()).unwrap();
    let root = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![
                PathOp::Add {
                    path: "empty".into(),
                    blob: empty_blob,
                    mode: 0o100_644,
                    status: EpistemicStatus::Stated,
                },
                PathOp::Add {
                    path: "readme".into(),
                    blob,
                    mode: 0o100_644,
                    status: EpistemicStatus::Stated,
                },
            ],
            copies: vec![],
            metadata: MetadataClaims {
                author: Some(crate::model::Identity {
                    name: crate::model::Text::utf8("A U Thor"),
                    email: Some(crate::model::Text::utf8("a@example.com")),
                }),
                author_time: Some(crate::model::Time {
                    seconds: 1_700_000_000,
                    offset_minutes: Some(60),
                }),
                committer: None,
                commit_time: None,
                message: Some(crate::model::Text::utf8("initial")),
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
            reason: "contract fixture".into(),
            step: None,
        }],
    });
    b.add_flag(Flag {
        kind: FlagKind::BelowConfidenceFloor,
        what: "an example flag".into(),
        count: 1,
        reason: "contract fixture".into(),
    });
    b.finish().unwrap()
}

fn fixture_data_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name)
}

/// RFC 018 §6.3: `to_bytes` must reproduce `contract-0.3.0.ir` byte for byte. A failure here means a
/// codec or writer change altered the file's bytes — note it in the CHANGELOG, and regenerate
/// (`UPDATE_GOLDEN=1 cargo test -p brygge-ir --lib golden_bytes_contract_0_3_0`).
#[test]
fn golden_bytes_contract_0_3_0() {
    let bytes = to_bytes(&fixture_ir());
    let path = fixture_data_path("contract-0.3.0.ir");
    if std::env::var("UPDATE_GOLDEN").is_ok() {
        std::fs::write(&path, &bytes).unwrap();
        return;
    }
    let golden = std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e} (run with UPDATE_GOLDEN=1 to create it)",
            path.display()
        )
    });
    assert_eq!(
        bytes,
        golden,
        "to_bytes's output no longer matches {}: a codec or writer change altered the artifact's \
         bytes. If intentional, note it in the CHANGELOG and regenerate the file.",
        path.display()
    );
}

/// Write `ir` as a **contract 0.2.0** artifact directly (the prior, unframed layout — `write_to` no
/// longer produces it, by design: RFC 018 D-7 keeps 0.2.0 read-only). Used only to freeze the committed
/// fixture below.
fn write_0_2_0(ir: &Ir) -> Vec<u8> {
    let mut version = CanonWriter::new();
    version.uvarint(0);
    version.uvarint(2);
    version.uvarint(0);

    let mut body = CanonWriter::new();
    let meta = ir.encode_metadata();
    body.uvarint(meta.len() as u64);
    body.raw(&meta);
    body.uvarint(ir.content.len() as u64);
    for id in ir.content.ids() {
        let bytes = ir.content.get(id).unwrap().unwrap();
        body.raw(id.as_bytes());
        body.uvarint(bytes.len() as u64);
        body.raw(&bytes);
    }

    let mut hasher = Sha256::new();
    hasher.update(version.as_bytes());
    hasher.update(body.as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    let mut out = MAGIC.to_vec();
    out.push(FORMAT);
    out.extend_from_slice(version.as_bytes());
    out.extend_from_slice(&digest);
    out.extend_from_slice(body.as_bytes());
    out
}

/// RFC 018 §6.4/D-7: the committed 0.2.0 fixture (the prior, unframed layout) still reads through both
/// entry points, its digest matches what is stored, and `contract` reports `0.2.0`.
#[test]
fn reads_the_0_2_0_fixture() {
    let path = fixture_data_path("contract-0.2.0.ir");
    if std::env::var("UPDATE_GOLDEN").is_ok() {
        std::fs::write(&path, write_0_2_0(&fixture_ir())).unwrap();
    }
    let bytes = std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e} (run with UPDATE_GOLDEN=1 to create it)",
            path.display()
        )
    });

    let via_from_bytes = from_bytes(&bytes).unwrap();
    assert_eq!(via_from_bytes.contract, ContractVersion::new(0, 2, 0));
    assert_eq!(via_from_bytes.ir, fixture_ir());

    let via_read_from = read_from(std::io::Cursor::new(bytes)).unwrap();
    assert_eq!(via_read_from.contract, ContractVersion::new(0, 2, 0));
    assert_eq!(via_read_from.digest, via_from_bytes.digest);
    assert_eq!(via_read_from.ir, via_from_bytes.ir);
}

/// RFC 018 §6.2 (digest independence): the same `Ir`, written once with DEFLATE frames (the normal
/// `to_bytes`) and once with every frame forced `Codec::Stored`, gives two different files but one
/// digest, and both decode back to an equal `Ir`.
#[test]
fn digest_is_independent_of_the_frame_codec() {
    let ir = fixture_ir();
    let deflated = to_bytes(&ir);

    let mut cursor = std::io::Cursor::new(Vec::new());
    write_to_stored_only(&ir, &mut cursor).unwrap();
    let stored_only = cursor.into_inner();

    assert_ne!(
        deflated, stored_only,
        "the fixture's repeated-word blob should actually compress smaller"
    );

    let a = from_bytes(&deflated).unwrap();
    let b = from_bytes(&stored_only).unwrap();
    assert_eq!(a.digest, b.digest);
    assert_eq!(a.ir, b.ir);
    assert_eq!(a.ir, ir);
}

/// RFC 018 §6.5: two frames that should have been one (an extra, needless frame boundary within the
/// 1 MiB cap) is refused as a packing-rule violation.
#[test]
fn wrong_packing_two_frames_that_should_be_one_is_rejected() {
    let ir = build();
    let order = store_order(&ir);
    let lens: Vec<u64> = order
        .iter()
        .map(|id| ir.content.get(id).unwrap().unwrap().len() as u64)
        .collect();
    // The real writer packs both of `build()`'s tiny blobs into one frame; split them into two here.
    assert_eq!(
        frame::canonical_frame_raw_lens(lens.iter().copied()).len(),
        1
    );
    let bytes_by_id: Vec<Vec<u8>> = order
        .iter()
        .map(|id| ir.content.get(id).unwrap().unwrap())
        .collect();

    let mut version = CanonWriter::new();
    version.uvarint(0);
    version.uvarint(3);
    version.uvarint(0);
    let meta_bytes = ir.encode_metadata();
    let mut hasher = Sha256::new();
    hasher.update(version.as_bytes());
    hasher.update(canon::uvarint_value(meta_bytes.len() as u64));
    hasher.update(&meta_bytes);
    hasher.update(canon::uvarint_value(order.len() as u64));
    for (id, len) in order.iter().zip(lens.iter()) {
        hasher.update(id.as_bytes());
        hasher.update(canon::uvarint_value(*len));
    }
    for bytes in &bytes_by_id {
        hasher.update(bytes);
    }
    let digest: [u8; 32] = hasher.finalize().into();

    let mut out = MAGIC.to_vec();
    out.push(FORMAT);
    out.extend_from_slice(version.as_bytes());
    out.extend_from_slice(&digest);
    write_frames(
        &mut out,
        &meta_bytes,
        &frame::fixed_chunk_lens(meta_bytes.len() as u64),
        false,
    )
    .unwrap();
    out.extend_from_slice(&canon::uvarint_value(order.len() as u64));
    for (id, len) in order.iter().zip(lens.iter()) {
        out.extend_from_slice(id.as_bytes());
        out.extend_from_slice(&canon::uvarint_value(*len));
    }
    // Two frames, one per blob, instead of the one canonical frame.
    out.extend_from_slice(&canon::uvarint_value(2));
    for bytes in &bytes_by_id {
        write_one_frame(&mut out, bytes, true).unwrap();
    }

    match from_bytes(&out) {
        Err(Error::NonCanonical(m)) => assert!(m.contains("packing"), "{m}"),
        other => panic!("expected NonCanonical (packing), got {other:?}"),
    }
}

/// RFC 018 §6.5: a blob table out of [`store_order`] is refused.
#[test]
fn a_blob_table_out_of_store_order_is_rejected() {
    let ir = build();
    let mut order = store_order(&ir);
    assert!(order.len() >= 2, "build() has at least two blobs");
    order.swap(0, 1); // no longer store_order's own order
    let lens: Vec<u64> = order
        .iter()
        .map(|id| ir.content.get(id).unwrap().unwrap().len() as u64)
        .collect();
    let bytes_by_id: Vec<Vec<u8>> = order
        .iter()
        .map(|id| ir.content.get(id).unwrap().unwrap())
        .collect();

    let mut version = CanonWriter::new();
    version.uvarint(0);
    version.uvarint(3);
    version.uvarint(0);
    let meta_bytes = ir.encode_metadata();
    let mut hasher = Sha256::new();
    hasher.update(version.as_bytes());
    hasher.update(canon::uvarint_value(meta_bytes.len() as u64));
    hasher.update(&meta_bytes);
    hasher.update(canon::uvarint_value(order.len() as u64));
    for (id, len) in order.iter().zip(lens.iter()) {
        hasher.update(id.as_bytes());
        hasher.update(canon::uvarint_value(*len));
    }
    for bytes in &bytes_by_id {
        hasher.update(bytes);
    }
    let digest: [u8; 32] = hasher.finalize().into();

    let mut out = MAGIC.to_vec();
    out.push(FORMAT);
    out.extend_from_slice(version.as_bytes());
    out.extend_from_slice(&digest);
    write_frames(
        &mut out,
        &meta_bytes,
        &frame::fixed_chunk_lens(meta_bytes.len() as u64),
        false,
    )
    .unwrap();
    out.extend_from_slice(&canon::uvarint_value(order.len() as u64));
    for (id, len) in order.iter().zip(lens.iter()) {
        out.extend_from_slice(id.as_bytes());
        out.extend_from_slice(&canon::uvarint_value(*len));
    }
    let frame_lens = frame::canonical_frame_raw_lens(lens.iter().copied());
    out.extend_from_slice(&canon::uvarint_value(frame_lens.len() as u64));
    let mut blob_iter = bytes_by_id.iter();
    for &flen in &frame_lens {
        let mut buf = Vec::new();
        let mut collected = 0u64;
        while collected < flen {
            let bytes = blob_iter.next().unwrap();
            collected += bytes.len() as u64;
            buf.extend_from_slice(bytes);
        }
        write_one_frame(&mut out, &buf, true).unwrap();
    }

    match from_bytes(&out) {
        Err(Error::NonCanonical(m)) => assert!(m.contains("store order"), "{m}"),
        other => panic!("expected NonCanonical (store order), got {other:?}"),
    }
}

/// RFC 018 §6.5: a `0.1` and a `0.4` artifact are both refused (too old for `READABLE`, too new).
#[test]
fn versions_outside_readable_are_refused() {
    let ir = build();
    for (major, minor, patch) in [(0u64, 1u64, 0u64), (0, 4, 0)] {
        let bytes = container_with_version(&ir, major, minor, patch);
        match from_bytes(&bytes) {
            Err(Error::UnsupportedContract { .. }) => {}
            other => {
                panic!("expected UnsupportedContract for {major}.{minor}.{patch}, got {other:?}")
            }
        }
    }
}

// ---- RFC 018, review 099 R-1: a decompression bomb must never reach an allocation it could abuse ----

fn peak_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

/// Best effort (Linux only, and not guaranteed to be writable under every sandbox): resets the
/// kernel's own peak-RSS mark, so a `VmHWM` read afterwards reflects what runs next, not whatever
/// came before it (the same technique `tools/bench` already uses). Returns `false` when unavailable,
/// so a caller can skip rather than fail.
fn reset_peak_rss() -> bool {
    std::fs::write("/proc/self/clear_refs", "5").is_ok()
}

const DEFLATE_BOMB_RAW_LEN: u64 = 256 * 1024 * 1024;

/// Switches [`deflate_bomb_peak_rss_helper`] on when set — otherwise it is a no-op, so it never runs,
/// or measures anything, as part of an ordinary `cargo test` pass. Only
/// [`a_one_gib_deflate_bomb_streams_through_read_from_within_a_bounded_peak`] sets it, on the child
/// process it spawns to run that one test alone (review 103 P-3).
const DEFLATE_BOMB_CHILD_ENV: &str = "BRYGGE_DEFLATE_BOMB_CHILD";

/// Builds the deflate-bomb artifact's bytes, without ever materializing its claimed 256 MiB: the
/// frame's compressed bytes come from `frame::deflate_zeros_streamed` (64 KiB chunks of zeros through
/// `miniz_oxide`'s own streaming compressor, one chunk held at a time), and the digest's content pass
/// hashes the same 256 MiB of zeros the same way, a chunk at a time. Hand-built from the logical
/// components directly (not by patching `to_bytes`'s own output): the table keeps the *real* blob id
/// (`store_order`'s own, matching the IR's blob reference, so the store-order check passes), but the
/// frame's actual content (a real, highly compressible DEFLATE stream of zeros) is not that id's — so
/// the content check, reached only after everything else passes, is the one that fails, only once the
/// whole claim has been inflated and hashed. The digest is computed over this same (real-id,
/// big-content) combination, so the pass reaches that far rather than failing on the digest.
fn deflate_bomb_bytes() -> Vec<u8> {
    let mut b = IrBuilder::new(provenance());
    // The real blob stays small: nothing else depends on its size, only that it is some real content
    // `real_id` legitimately names — the content-address mismatch below comes from the big blob's frame
    // simply being a different, much larger payload, not from any particular byte value chosen for it.
    let real_id = b.add_blob(b"small".to_vec()).unwrap();
    b.add_atom(AtomDraft {
        parents: vec![],
        ops: vec![PathOp::Add {
            path: "big".into(),
            blob: real_id,
            mode: 0o100_644,
            status: EpistemicStatus::Stated,
        }],
        copies: vec![],
        metadata: MetadataClaims::default(),
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
    let ir = b.finish().unwrap();
    let meta_bytes = ir.encode_metadata();
    drop(ir);

    let stored = frame::deflate_zeros_streamed(DEFLATE_BOMB_RAW_LEN);

    let mut version = CanonWriter::new();
    version.uvarint(0);
    version.uvarint(3);
    version.uvarint(0);
    let mut hasher = Sha256::new();
    hasher.update(version.as_bytes());
    hasher.update(canon::uvarint_value(meta_bytes.len() as u64));
    hasher.update(&meta_bytes);
    hasher.update(canon::uvarint_value(1)); // one blob
    hasher.update(real_id.as_bytes());
    hasher.update(canon::uvarint_value(DEFLATE_BOMB_RAW_LEN));
    let zero_chunk = [0u8; 64 * 1024];
    let mut remaining = DEFLATE_BOMB_RAW_LEN;
    while remaining > 0 {
        let n = remaining.min(zero_chunk.len() as u64) as usize;
        hasher.update(&zero_chunk[..n]);
        remaining -= n as u64;
    }
    let digest: [u8; 32] = hasher.finalize().into();

    let mut bytes = MAGIC.to_vec();
    bytes.push(FORMAT);
    bytes.extend_from_slice(version.as_bytes());
    bytes.extend_from_slice(&digest);
    write_frames(
        &mut bytes,
        &meta_bytes,
        &frame::fixed_chunk_lens(meta_bytes.len() as u64),
        false,
    )
    .unwrap();
    bytes.extend_from_slice(&canon::uvarint_value(1));
    bytes.extend_from_slice(real_id.as_bytes());
    bytes.extend_from_slice(&canon::uvarint_value(DEFLATE_BOMB_RAW_LEN));
    bytes.extend_from_slice(&canon::uvarint_value(1)); // one blob frame
    write_frame_body(
        &mut bytes,
        frame::Codec::Deflate,
        DEFLATE_BOMB_RAW_LEN,
        &stored,
    )
    .unwrap();
    bytes
}

/// Review 103 P-3: does the actual decode, in whatever process runs it, and prints this process's own
/// peak-RSS delta to stdout as `DEFLATE_BOMB_DELTA_KIB=<n>` (or `=unavailable` if `/proc/self/clear_refs`
/// could not be reset) — never asserted here directly, since a no-op run (the env switch unset) must
/// exit cleanly with nothing to report.
/// `a_one_gib_deflate_bomb_streams_through_read_from_within_a_bounded_peak` (below) is the only caller
/// that ever sets the switch, on a freshly spawned, single-threaded child process running this one test
/// alone — immune to whatever every *other* test thread in a shared process is
/// concurrently allocating, which is what made the old, same-process measurement flaky (review 103 P-3,
/// the architect's own miss in review 101: `VmHWM` is process-wide).
#[test]
fn deflate_bomb_peak_rss_helper() {
    if std::env::var_os(DEFLATE_BOMB_CHILD_ENV).is_none() {
        return;
    }
    let bytes = deflate_bomb_bytes();
    let dir = scratch_dir("deflate-bomb-child");
    let path = dir.join("artifact");
    std::fs::write(&path, &bytes).unwrap();

    if !reset_peak_rss() {
        let _ = std::fs::remove_dir_all(&dir);
        println!("DEFLATE_BOMB_DELTA_KIB=unavailable");
        return;
    }
    let before = peak_rss_kib();
    let err = from_file(&path).unwrap_err();
    let after = peak_rss_kib();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        matches!(err, Error::Decode(ref m) if m.contains("does not match its id")),
        "expected a content-address refusal, got {err:?}"
    );
    match (before, after) {
        (Some(before), Some(after)) => {
            println!("DEFLATE_BOMB_DELTA_KIB={}", after.saturating_sub(before));
        }
        _ => println!("DEFLATE_BOMB_DELTA_KIB=unavailable"),
    }
}

/// Review 099 R-1's own proof. Review 101 F-1: review 100's resize to 4 MiB (to fit `mutation.yml`'s
/// own `ulimit -v 2000000` deep-sweep cap) let this test pass code review 099 itself found broken — a
/// whole-buffer inflate that allocates `raw_len` up front also fits comfortably under 4 MiB (well under
/// the 16 MiB bound below), so a test at that size proved nothing about the bug it exists to catch. Back
/// to a size that bug cannot survive, 256 MiB (`deflate_bomb_bytes`, above).
///
/// **Review 103 P-3** (the architect's own miss in review 101): the peak-RSS measurement itself used to
/// run inline, in this same test process — but `VmHWM` (what `peak_rss_kib` reads) is process-wide, so
/// under `--test-threads=4` every *other* test thread's own concurrent allocations counted against this
/// one test's 16 MiB bound, making it flaky by design, not a property of this batch's own changes. Fixed
/// by re-running this binary as a child process (`std::env::current_exe()`), filtered with `--exact` to
/// run only [`deflate_bomb_peak_rss_helper`], alone, at `--test-threads=1` — the measurement now reflects
/// that one decode, in a process nothing else is concurrently allocating in.
#[test]
fn a_one_gib_deflate_bomb_streams_through_read_from_within_a_bounded_peak() {
    let exe = std::env::current_exe().expect("this test's own binary has a path");
    let output = std::process::Command::new(&exe)
        .args([
            "artifact::tests::deflate_bomb_peak_rss_helper",
            "--exact",
            "--test-threads=1",
            "--nocapture",
        ])
        .env(DEFLATE_BOMB_CHILD_ENV, "1")
        .output()
        .expect("spawn the child measurement process");
    assert!(
        output.status.success(),
        "the child measurement process failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Not `.lines().find_map(strip_prefix(...))`: with `--nocapture`, libtest writes its own
    // "test <name> ... " progress text *before* the newline that would otherwise end that line, so a
    // test's own `println!` lands appended to that same line, never at its start. Searching the whole
    // text for the marker, wherever it falls, is what actually finds it.
    let after_marker = stdout
        .split("DEFLATE_BOMB_DELTA_KIB=")
        .nth(1)
        .unwrap_or_else(|| panic!("no DEFLATE_BOMB_DELTA_KIB marker in child output: {stdout}"));
    let delta_line = after_marker
        .split(|c: char| c.is_whitespace())
        .next()
        .unwrap_or("");

    if delta_line == "unavailable" {
        if cfg!(target_os = "linux") {
            // Review 100 S-3: on Linux, `/proc/self/clear_refs` missing or unwritable is the test
            // environment failing to provide what this check needs, not a legitimate reason to skip
            // (review 093's same ruling for the test plugin) — this is the one OS every CI job that
            // runs this test runs on.
            panic!(
                "/proc/self/clear_refs is not writable in the child process, and this is Linux: the \
                 peak-RSS bound cannot be checked"
            );
        }
        eprintln!(
            "skipping: /proc/self/clear_refs is not writable in the child process (not Linux)"
        );
        return;
    }
    let delta: u64 = delta_line
        .parse()
        .unwrap_or_else(|e| panic!("malformed DEFLATE_BOMB_DELTA_KIB value {delta_line:?}: {e}"));
    assert!(
        delta < 16 * 1024,
        "the child process's own peak grew by {delta} KiB above baseline, inflating a 256 MiB claim; \
         expected well under 16 MiB"
    );
}

/// Review 099 R-1: a blob frame whose `raw_len` does not match the packing plan is refused — the
/// specific "does not match the packing rule" `NonCanonical` — *before* its stored bytes are read at
/// all: the artifact is truncated right where those bytes would start, so reaching for them would give
/// a different, truncation-shaped error instead.
#[test]
fn a_frame_whose_raw_len_disagrees_with_the_plan_is_refused_before_its_stored_bytes_are_read() {
    let ir = build();

    // Rebuilt by hand (rather than mutating `to_bytes`'s output in place) so the corrupted header's
    // raw_len is at a known offset, with no stored bytes at all following it — if the plan check ran
    // only after an attempt to read those (missing) bytes, this would give a truncation error instead
    // of the specific packing-rule one.
    let order = store_order(&ir);
    let lens: Vec<u64> = order
        .iter()
        .map(|id| ir.content.get(id).unwrap().unwrap().len() as u64)
        .collect();
    let bytes_by_id: Vec<Vec<u8>> = order
        .iter()
        .map(|id| ir.content.get(id).unwrap().unwrap())
        .collect();
    let total: u64 = lens.iter().sum();

    let mut version = CanonWriter::new();
    version.uvarint(0);
    version.uvarint(3);
    version.uvarint(0);
    let meta_bytes = ir.encode_metadata();
    let mut hasher = Sha256::new();
    hasher.update(version.as_bytes());
    hasher.update(canon::uvarint_value(meta_bytes.len() as u64));
    hasher.update(&meta_bytes);
    hasher.update(canon::uvarint_value(order.len() as u64));
    for (id, len) in order.iter().zip(lens.iter()) {
        hasher.update(id.as_bytes());
        hasher.update(canon::uvarint_value(*len));
    }
    for bytes in &bytes_by_id {
        hasher.update(bytes);
    }
    let digest: [u8; 32] = hasher.finalize().into();

    let mut out = MAGIC.to_vec();
    out.push(FORMAT);
    out.extend_from_slice(version.as_bytes());
    out.extend_from_slice(&digest);
    write_frames(
        &mut out,
        &meta_bytes,
        &frame::fixed_chunk_lens(meta_bytes.len() as u64),
        false,
    )
    .unwrap();
    out.extend_from_slice(&canon::uvarint_value(order.len() as u64));
    for (id, len) in order.iter().zip(lens.iter()) {
        out.extend_from_slice(id.as_bytes());
        out.extend_from_slice(&canon::uvarint_value(*len));
    }
    // One blob frame, header only: codec 0 (stored), a `raw_len`/`stored_len` that disagree with the
    // plan (`total`) by claiming one byte more — then stop. No stored bytes follow at all.
    out.extend_from_slice(&canon::uvarint_value(1));
    out.extend_from_slice(&canon::uvarint_value(0));
    out.extend_from_slice(&canon::uvarint_value(total + 1));
    out.extend_from_slice(&canon::uvarint_value(total + 1));

    match from_bytes(&out) {
        Err(Error::NonCanonical(m)) => assert!(
            m.contains("does not match the packing rule"),
            "expected the packing-rule refusal, got: {m}"
        ),
        other => panic!("expected NonCanonical (packing rule), got {other:?}"),
    }
}

// ---- RFC 018, review 099 R-6: parallel frame compression is byte-identical to the serial writer -----

/// Many small blobs (well past the 64 MiB parallel window, so `write_blob_frames` must flush and
/// resume more than once) and a few large, each-their-own-frame blobs — exercising both the parallel
/// (packed, ≤1 MiB) and solo (>1 MiB) paths in one `Ir`.
fn many_frame_ir() -> Ir {
    let mut b = IrBuilder::new(provenance());
    let mut ops = Vec::new();
    // Enough 200 KiB blobs to need several 64 MiB windows (80 × 200 KiB ≈ 15.6 MiB — kept small enough
    // for a fast test while still crossing more than one packed frame boundary; the window-crossing
    // logic itself is exercised by `PARALLEL_WINDOW`-sized accounting, not by the absolute byte count).
    for i in 0..80 {
        let mut content = format!("blob {i} of many, ").into_bytes();
        content.resize(200 * 1024, b'.');
        let blob = b.add_blob(content).unwrap();
        ops.push(PathOp::Add {
            path: format!("file-{i:03}"),
            blob,
            mode: 0o100_644,
            status: EpistemicStatus::Stated,
        });
    }
    for i in 0..3 {
        let content = vec![i as u8; 2 * 1024 * 1024]; // > 1 MiB: its own frame
        let blob = b.add_blob(content).unwrap();
        ops.push(PathOp::Add {
            path: format!("large-{i}"),
            blob,
            mode: 0o100_644,
            status: EpistemicStatus::Stated,
        });
    }
    b.add_atom(AtomDraft {
        parents: vec![],
        ops,
        copies: vec![],
        metadata: MetadataClaims::default(),
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
    b.finish().unwrap()
}

#[test]
fn parallel_blob_frame_compression_is_byte_identical_to_the_serial_writer() {
    let ir = many_frame_ir();
    let order = store_order(&ir);
    let lens: Vec<u64> = order
        .iter()
        .map(|id| ir.content.get(id).unwrap().unwrap().len() as u64)
        .collect();
    let frame_lens = frame::canonical_frame_raw_lens(lens.iter().copied());
    assert!(
        frame_lens.len() > 4,
        "this fixture should need several frames"
    );

    // The real writer (parallel): `write_blob_frames`, via `write_to`/`to_bytes`.
    let parallel = to_bytes(&ir);

    // A purely serial re-implementation of the same blob-frame loop, kept independent of
    // `write_blob_frames` on purpose (so a bug shared by both would not cancel out): one frame's raw
    // bytes assembled, compressed, and written, one at a time, exactly as the pre-R-6 writer did.
    let mut version = CanonWriter::new();
    version.uvarint(0);
    version.uvarint(3);
    version.uvarint(0);
    let meta_bytes = ir.encode_metadata();
    let mut hasher = Sha256::new();
    hasher.update(version.as_bytes());
    hasher.update(canon::uvarint_value(meta_bytes.len() as u64));
    hasher.update(&meta_bytes);
    hasher.update(canon::uvarint_value(order.len() as u64));
    for (id, len) in order.iter().zip(lens.iter()) {
        hasher.update(id.as_bytes());
        hasher.update(canon::uvarint_value(*len));
    }
    let mut blob_iter = order.iter();
    let mut blob_frame_bytes = Vec::new();
    blob_frame_bytes.extend_from_slice(&canon::uvarint_value(frame_lens.len() as u64));
    for &flen in &frame_lens {
        let mut buf = Vec::new();
        let mut collected = 0u64;
        while collected < flen {
            let id = blob_iter.next().unwrap();
            let bytes = ir.content.get(id).unwrap().unwrap();
            hasher.update(&bytes);
            collected += bytes.len() as u64;
            buf.extend_from_slice(&bytes);
        }
        write_one_frame(&mut blob_frame_bytes, &buf, false).unwrap();
    }
    let digest: [u8; 32] = hasher.finalize().into();

    let mut serial = MAGIC.to_vec();
    serial.push(FORMAT);
    serial.extend_from_slice(version.as_bytes());
    serial.extend_from_slice(&digest);
    write_frames(
        &mut serial,
        &meta_bytes,
        &frame::fixed_chunk_lens(meta_bytes.len() as u64),
        false,
    )
    .unwrap();
    serial.extend_from_slice(&canon::uvarint_value(order.len() as u64));
    for (id, len) in order.iter().zip(lens.iter()) {
        serial.extend_from_slice(id.as_bytes());
        serial.extend_from_slice(&canon::uvarint_value(*len));
    }
    serial.extend_from_slice(&blob_frame_bytes);

    assert_eq!(
        parallel, serial,
        "the parallel writer's bytes differ from an independent serial re-implementation"
    );

    // And, of course, still a valid, equal-IR artifact either way.
    let decoded = from_bytes(&parallel).unwrap();
    assert_eq!(decoded.ir, ir);
}

// ---- RFC 018 D-5: transform records (batch T) --------------------------------------------------

/// A two-record transform chain exercising every shape §8's proof asks for in one fixture:
/// `Transformed` atoms, ops and refs; a `Transformed` whose own `prior` is itself `Transformed`,
/// nested over a `Derived` one; and `Transform`-class drops at both steps.
fn transform_chain_ir() -> Ir {
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
            metadata: MetadataClaims::default(),
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
    let step0 = b.add_transform(TransformRecord {
        name: "fix-mode".into(),
        version: "1.0.0".into(),
        params: BTreeMap::new(),
        consumed: [0xaa; 32],
    });
    let step1 = b.add_transform(TransformRecord {
        name: "renumber-author".into(),
        version: "2.3.1".into(),
        params: BTreeMap::from([("rule".to_string(), "mailmap".to_string())]),
        consumed: [0xbb; 32],
    });
    assert_eq!((step0, step1), (0, 1));
    let child_blob = b.add_blob(b"second revision".to_vec()).unwrap();
    let child = b
        .add_atom(AtomDraft {
            parents: vec![root],
            ops: vec![PathOp::Add {
                path: "second".into(),
                blob: child_blob,
                mode: 0o100_755,
                status: EpistemicStatus::Transformed {
                    step: step0 as u64,
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
            // A Transformed with a Transformed prior over a Derived one.
            status: EpistemicStatus::Transformed {
                step: step1 as u64,
                changed: vec![ChangedPart::Ops],
                prior: Box::new(EpistemicStatus::Transformed {
                    step: step0 as u64,
                    changed: vec![ChangedPart::Author],
                    prior: Box::new(EpistemicStatus::Derived(crate::status::Derivation {
                        kind: crate::status::DerivationKind::NormalizedMetadata,
                        by: "brygge-decode-git".into(),
                        decoder_version: "0.1.0".into(),
                        params: BTreeMap::new(),
                        confidence: None,
                    })),
                }),
            },
        })
        .unwrap();
    b.add_ref(RefRecord {
        name: "refs/heads/main".into(),
        kind: RefKind::Branch,
        target: child,
        status: EpistemicStatus::Transformed {
            step: step1 as u64,
            changed: vec![ChangedPart::Target],
            prior: Box::new(EpistemicStatus::Stated),
        },
        source: None,
        annotation: None,
    })
    .unwrap();
    b.set_loss(LossBoundary {
        dropped: vec![
            DropRecord {
                class: LossClass::Transform,
                what: format!("op {} workflow-state.txt", root.to_hex()),
                reason: "step 0 dropped it".into(),
                step: Some(step0 as u64),
            },
            DropRecord {
                class: LossClass::Transform,
                what: "ref branch refs/heads/alias".into(),
                reason: "step 1 dropped it".into(),
                step: Some(step1 as u64),
            },
        ],
    });
    b.finish().unwrap()
}

/// RFC 018 D-5, §8 item 1: a two-record chain — `Transformed` atoms, ops and refs; a `Transformed`
/// nested over a `Transformed` nested over a `Derived`; `Transform` drops at both steps — round-trips
/// through both readers, and `digest_of` agrees with what each one verified.
#[test]
fn transform_chain_round_trips_through_both_readers_and_digest_of_agrees() {
    let ir = transform_chain_ir();
    let bytes = to_bytes(&ir);

    let via_from_bytes = from_bytes(&bytes).unwrap();
    assert_eq!(via_from_bytes.ir, ir);

    let via_read_from = read_from(std::io::Cursor::new(bytes)).unwrap();
    assert_eq!(via_read_from.ir, ir);
    assert_eq!(via_read_from.digest, via_from_bytes.digest);

    assert_eq!(digest_of(&ir).unwrap(), via_from_bytes.digest);
}

/// RFC 018 D-3/D-5: `digest_of` agrees with the digest each reader itself verified, for every shape
/// this module's own fixtures cover — not only the transform-chain one above.
#[test]
fn digest_of_agrees_with_decoded_digest_for_every_fixture() {
    for ir in [
        build(),
        build_rich(),
        fixture_ir(),
        many_frame_ir(),
        transform_chain_ir(),
    ] {
        let decoded = from_bytes(&to_bytes(&ir)).unwrap();
        assert_eq!(digest_of(&ir).unwrap(), decoded.digest);
    }
}

/// RFC 018 D-5 §2: at most 64 transform records — a 65th is refused, from both readers.
#[test]
fn more_than_64_transform_records_is_rejected() {
    let ir = Ir {
        atoms: vec![],
        refs: vec![],
        provenance: provenance(),
        loss: LossBoundary::default(),
        flags: vec![],
        content: crate::content::ContentStore::new(),
        transforms: (0..65u16)
            .map(|i| TransformRecord {
                name: "t".into(),
                version: "1".into(),
                params: BTreeMap::new(),
                consumed: [i as u8; 32],
            })
            .collect(),
    };
    let bytes = to_bytes(&ir);
    match from_bytes(&bytes) {
        Err(Error::NonCanonical(m)) => assert!(m.contains("64"), "{m}"),
        other => panic!("expected NonCanonical, got {other:?}"),
    }
    match read_from(std::io::Cursor::new(bytes)) {
        Err(Error::NonCanonical(m)) => assert!(m.contains("64"), "{m}"),
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

/// RFC 018 D-5 §2: an op's `Transformed.changed` may only use the op vocabulary (`blob`/`mode`/
/// `path`) — using an atom-only part (`author`) is refused.
#[test]
fn an_ops_changed_using_the_atom_vocabulary_is_rejected() {
    let mut b = IrBuilder::new(provenance());
    let blob = b.add_blob(b"x".to_vec()).unwrap();
    let step = b.add_transform(TransformRecord {
        name: "t".into(),
        version: "1".into(),
        params: BTreeMap::new(),
        consumed: [0u8; 32],
    });
    b.add_atom(AtomDraft {
        parents: vec![],
        ops: vec![PathOp::Add {
            path: "p".into(),
            blob,
            mode: 0o100_644,
            status: EpistemicStatus::Transformed {
                step: step as u64,
                changed: vec![ChangedPart::Author], // valid on an atom, not on an op
                prior: Box::new(EpistemicStatus::Stated),
            },
        }],
        copies: vec![],
        metadata: MetadataClaims::default(),
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
    let ir = b.finish().unwrap();
    let bytes = to_bytes(&ir);
    match from_bytes(&bytes) {
        Err(Error::Decode(m)) => assert!(m.contains("vocabulary"), "{m}"),
        other => panic!("expected Decode, got {other:?}"),
    }
}

/// RFC 018 D-5: a `DropRecord.step` is refused on any class but `Transform` (model.rs's own decode
/// check) — a `Representation` drop with a `step` is never canonical.
#[test]
fn a_step_on_a_representation_drop_is_rejected() {
    let ir = Ir {
        atoms: vec![],
        refs: vec![],
        provenance: provenance(),
        loss: LossBoundary {
            dropped: vec![DropRecord {
                class: LossClass::Representation,
                what: "packfiles".into(),
                reason: "representation, not assertion".into(),
                step: Some(0),
            }],
        },
        flags: vec![],
        content: crate::content::ContentStore::new(),
        transforms: vec![TransformRecord {
            name: "t".into(),
            version: "1".into(),
            params: BTreeMap::new(),
            consumed: [0u8; 32],
        }],
    };
    let bytes = to_bytes(&ir);
    match from_bytes(&bytes) {
        Err(Error::NonCanonical(m)) => assert!(m.contains("step"), "{m}"),
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}

/// RFC 018 D-5 §2: a 0.2.0 artifact holding `Ir` field 6 (`transforms`) is refused — field 6 does not
/// exist in 0.2.0, and the shared metadata decoder must know the contract it reads.
#[test]
fn field_6_in_a_0_2_0_artifact_is_rejected() {
    let ir = transform_chain_ir();
    let bytes = write_0_2_0(&ir);
    match from_bytes(&bytes) {
        Err(Error::NonCanonical(m)) => assert!(m.contains("field 6"), "{m}"),
        other => panic!("expected NonCanonical, got {other:?}"),
    }
    match read_from(std::io::Cursor::new(bytes)) {
        Err(Error::NonCanonical(m)) => assert!(m.contains("field 6"), "{m}"),
        other => panic!("expected NonCanonical, got {other:?}"),
    }
}
