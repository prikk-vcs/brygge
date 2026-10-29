//! Tests for the frame codec and packing plan (RFC 018 D-1).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;

#[test]
fn compress_then_inflate_round_trips() {
    let raw = b"hello world, hello world, hello world, hello world".repeat(100);
    let (codec, stored) = compress_frame(&raw);
    assert_eq!(codec, Codec::Deflate, "repetitive input compresses smaller");
    assert!(stored.len() < raw.len());
    let back = inflate_exact(codec, &stored, raw.len() as u64).unwrap();
    assert_eq!(back, raw);
}

#[test]
fn incompressible_input_falls_back_to_stored() {
    // A single byte: DEFLATE's own framing overhead is never smaller than storing it as is.
    let raw = vec![7u8];
    let (codec, stored) = compress_frame(&raw);
    assert_eq!(codec, Codec::Stored);
    assert_eq!(stored, raw);
    let back = inflate_exact(codec, &stored, raw.len() as u64).unwrap();
    assert_eq!(back, raw);
}

#[test]
fn stored_codec_requires_exact_length() {
    let err = inflate_exact(Codec::Stored, b"abc", 4).unwrap_err();
    assert!(matches!(err, crate::Error::Decode(_)));
}

#[test]
fn deflate_cannot_claim_past_the_expansion_bound() {
    // A tiny stored payload claiming an enormous raw_len is refused before any allocation.
    let err = inflate_exact(Codec::Deflate, &[0x00], 1 << 30).unwrap_err();
    assert!(matches!(err, crate::Error::Decode(_)));
}

#[test]
fn deflate_short_or_overlong_output_is_refused() {
    let raw = b"some real content to compress, repeated, some real content to compress".repeat(10);
    let (codec, stored) = compress_frame(&raw);
    assert_eq!(codec, Codec::Deflate);
    assert!(inflate_exact(codec, &stored, raw.len() as u64 - 1).is_err());
    assert!(inflate_exact(codec, &stored, raw.len() as u64 + 1).is_err());
}

#[test]
fn unknown_codec_is_refused() {
    assert!(Codec::from_u64(2).is_err());
}

#[test]
fn canonical_frame_raw_lens_packs_small_blobs_together() {
    let lens = vec![100u64, 200, 300];
    assert_eq!(canonical_frame_raw_lens(lens), vec![600]);
}

#[test]
fn canonical_frame_raw_lens_splits_at_the_cap() {
    let lens = vec![FRAME_CAP - 10, 20, 5];
    // The second blob (20 bytes) does not fit in the first frame (only 10 bytes left), so it starts a
    // new frame; the third (5 bytes) joins that new frame.
    assert_eq!(canonical_frame_raw_lens(lens), vec![FRAME_CAP - 10, 25]);
}

#[test]
fn canonical_frame_raw_lens_gives_a_large_blob_its_own_frame() {
    let lens = vec![10u64, FRAME_CAP + 1, 20];
    assert_eq!(canonical_frame_raw_lens(lens), vec![10, FRAME_CAP + 1, 20]);
}

#[test]
fn canonical_frame_raw_lens_opens_a_frame_for_a_leading_empty_blob() {
    // An empty blob with no frame yet open opens one of its own (RFC 018 D-1), which later small
    // blobs then join.
    let lens = vec![0u64, 5, 5];
    assert_eq!(canonical_frame_raw_lens(lens), vec![10]);
}

#[test]
fn fixed_chunk_lens_cuts_metadata_at_exactly_one_mebibyte() {
    assert_eq!(fixed_chunk_lens(0), Vec::<u64>::new());
    assert_eq!(fixed_chunk_lens(10), vec![10]);
    assert_eq!(fixed_chunk_lens(FRAME_CAP), vec![FRAME_CAP]);
    assert_eq!(fixed_chunk_lens(FRAME_CAP + 1), vec![FRAME_CAP, 1]);
    assert_eq!(fixed_chunk_lens(2 * FRAME_CAP), vec![FRAME_CAP, FRAME_CAP]);
}
