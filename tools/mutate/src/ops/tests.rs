#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::{
    Boundary, Encoding, InterestingPosition, apply_boundary, apply_generic, apply_truncate,
};
use crate::rng::SplitMix64;

#[test]
fn a_binary_boundary_overwrites_in_place_without_changing_length() {
    let data = [0u8; 8];
    let pos = InterestingPosition {
        offset: 2,
        encoding: Encoding::Binary {
            width: 4,
            big_endian: true,
        },
        ceiling: Some(100),
    };
    let out = apply_boundary(&data, &pos, Boundary::Ceiling);
    assert_eq!(out.len(), data.len());
    assert_eq!(&out[2..6], &100u32.to_be_bytes());
}

#[test]
fn a_binary_boundary_respects_endianness() {
    let data = [0u8; 4];
    let pos = InterestingPosition {
        offset: 0,
        encoding: Encoding::Binary {
            width: 4,
            big_endian: false,
        },
        ceiling: None,
    };
    let out = apply_boundary(&data, &pos, Boundary::One);
    assert_eq!(out, 1u32.to_le_bytes());
}

#[test]
fn ceiling_plus_one_is_one_more_than_ceiling() {
    let data = [0u8; 4];
    let pos = InterestingPosition {
        offset: 0,
        encoding: Encoding::Binary {
            width: 2,
            big_endian: true,
        },
        ceiling: Some(10),
    };
    let out = apply_boundary(&data, &pos, Boundary::CeilingPlus1);
    assert_eq!(u16::from_be_bytes([out[0], out[1]]), 11);
}

#[test]
fn a_decimal_boundary_can_grow_the_buffer() {
    // b"5" (one digit) becomes b"18446744073709551615" (20 digits) at Max.
    let data = b"x5y".to_vec();
    let pos = InterestingPosition {
        offset: 1,
        encoding: Encoding::Decimal { len: 1 },
        ceiling: None,
    };
    let out = apply_boundary(&data, &pos, Boundary::Max);
    assert_eq!(out, b"x18446744073709551615y");
}

#[test]
fn a_uvarint_boundary_encodes_minimal_leb128() {
    // 300 encodes as [0xAC, 0x02] in minimal LEB128.
    let data = b"x\x05y".to_vec(); // a one-byte varint ("5") between two markers
    let pos = InterestingPosition {
        offset: 1,
        encoding: Encoding::Uvarint { len: 1 },
        ceiling: None,
    };
    let out = apply_boundary(&data, &pos, Boundary::One);
    assert_eq!(out, b"x\x01y");

    // Max grows the span: u64::MAX is 10 bytes in LEB128.
    let out = apply_boundary(&data, &pos, Boundary::Max);
    assert_eq!(out.first().copied(), Some(b'x'));
    assert_eq!(out.last().copied(), Some(b'y'));
    assert_eq!(out.len(), 1 + 10 + 1);
}

#[test]
fn a_big_endian_varint_boundary_matches_svndiffs_own_reader() {
    // A tiny reimplementation of svndiff's own varint reader (big-endian base-128, continuation bit on
    // every byte but the last), so this proves round-tripping against the real convention, not just
    // against itself.
    fn read(mut bytes: &[u8]) -> u64 {
        let mut value = 0u64;
        loop {
            let b = bytes[0];
            bytes = &bytes[1..];
            value = value * 128 + u64::from(b & 0x7f);
            if b & 0x80 == 0 {
                return value;
            }
        }
    }

    let data = b"x\x05y".to_vec();
    let pos = InterestingPosition {
        offset: 1,
        encoding: Encoding::BigEndianVarint { len: 1 },
        ceiling: Some(1000),
    };
    for (boundary, expected) in [
        (Boundary::Zero, 0u64),
        (Boundary::One, 1),
        (Boundary::Ceiling, 1000),
        (Boundary::CeilingPlus1, 1001),
    ] {
        let out = apply_boundary(&data, &pos, boundary);
        assert_eq!(out.first().copied(), Some(b'x'));
        assert_eq!(out.last().copied(), Some(b'y'));
        assert_eq!(read(&out[1..out.len() - 1]), expected);
    }
}

#[test]
fn a_boundary_past_the_end_leaves_the_data_unchanged() {
    let data = [0u8; 2];
    let pos = InterestingPosition {
        offset: 10,
        encoding: Encoding::Binary {
            width: 4,
            big_endian: true,
        },
        ceiling: None,
    };
    assert_eq!(apply_boundary(&data, &pos, Boundary::Max), data.to_vec());
}

#[test]
fn generic_operators_never_panic_on_a_tiny_or_empty_seed() {
    let mut rng = SplitMix64::new(99);
    for seed in [&b""[..], &b"a"[..], &b"ab"[..], &b"hello, world"[..]] {
        for _ in 0..200 {
            let (_out, name) = apply_generic(seed, b"other", &mut rng);
            assert!(!name.is_empty());
        }
        let (_out, name) = apply_truncate(seed, &mut rng);
        assert!(!name.is_empty());
    }
}

#[test]
fn boundaries_for_includes_ceiling_values_only_when_a_ceiling_is_declared() {
    let with = super::boundaries_for(&InterestingPosition {
        offset: 0,
        encoding: Encoding::Binary {
            width: 1,
            big_endian: true,
        },
        ceiling: Some(5),
    });
    let without = super::boundaries_for(&InterestingPosition {
        offset: 0,
        encoding: Encoding::Binary {
            width: 1,
            big_endian: true,
        },
        ceiling: None,
    });
    assert!(with.len() > without.len());
}
