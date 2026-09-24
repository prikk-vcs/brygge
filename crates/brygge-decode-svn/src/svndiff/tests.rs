//! Tests for the svndiff (version 0) reader: each opcode, a run, several windows, an empty target, every
//! malformed case, the version bytes, the ceiling, and a seeded property test (no panic, no growth past the
//! ceiling) over random bytes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::{DiffError, apply, check_header};

/// A base-128 big-endian integer.
fn varint(mut n: u64) -> Vec<u8> {
    let mut groups = vec![(n & 0x7f) as u8];
    n >>= 7;
    while n > 0 {
        groups.push((n & 0x7f) as u8 | 0x80);
        n >>= 7;
    }
    groups.reverse();
    groups
}

const HEADER: &[u8] = b"SVN\0";

/// One window: source view `(offset, len)`, the instructions and the new data; the target length is what the
/// instructions produce, given by the caller.
fn window(source: (u64, u64), target_len: u64, instructions: &[u8], new_data: &[u8]) -> Vec<u8> {
    let mut w = Vec::new();
    for n in [
        source.0,
        source.1,
        target_len,
        instructions.len() as u64,
        new_data.len() as u64,
    ] {
        w.extend(varint(n));
    }
    w.extend(instructions);
    w.extend(new_data);
    w
}

fn delta(windows: &[Vec<u8>]) -> Vec<u8> {
    let mut d = HEADER.to_vec();
    for w in windows {
        d.extend(w);
    }
    d
}

/// An instruction: opcode `op` (0 source, 1 target, 2 new), length, and for a copy the offset.
fn ins(op: u8, len: usize, offset: Option<u64>) -> Vec<u8> {
    let mut i = Vec::new();
    if (1..64).contains(&len) {
        i.push((op << 6) | len as u8);
    } else {
        i.push(op << 6);
        i.extend(varint(len as u64));
    }
    if let Some(o) = offset {
        i.extend(varint(o));
    }
    i
}

const BIG: usize = 1 << 20;

fn apply_ok(d: &[u8], base: &[u8]) -> Vec<u8> {
    apply(d, base, BIG).unwrap()
}

fn malformed(d: &[u8], base: &[u8]) -> String {
    match apply(d, base, BIG) {
        Err(DiffError::Malformed(m)) => m,
        other => panic!("expected Malformed, got {other:?}"),
    }
}

#[test]
fn a_new_data_instruction_writes_its_bytes() {
    let d = delta(&[window((0, 0), 5, &ins(2, 5, None), b"hello")]);
    assert_eq!(apply_ok(&d, b""), b"hello");
}

#[test]
fn a_source_copy_reads_the_source_view() {
    let base = b"0123456789";
    // the view is base[2..8] = "234567"; copy 3 bytes from view offset 1 -> "345"
    let d = delta(&[window((2, 6), 3, &ins(0, 3, Some(1)), b"")]);
    assert_eq!(apply_ok(&d, base), b"345");
}

#[test]
fn a_target_copy_may_overlap_what_it_writes_so_a_run_is_one_instruction() {
    // "ab" then copy 6 from target offset 0: the copy overlaps its own output -> "abababab".
    let mut instructions = ins(2, 2, None);
    instructions.extend(ins(1, 6, Some(0)));
    let d = delta(&[window((0, 0), 8, &instructions, b"ab")]);
    assert_eq!(apply_ok(&d, b""), b"abababab");
    // a run of one byte
    let mut instructions = ins(2, 1, None);
    instructions.extend(ins(1, 99, Some(0)));
    let d = delta(&[window((0, 0), 100, &instructions, b"z")]);
    assert_eq!(apply_ok(&d, b""), vec![b'z'; 100]);
}

#[test]
fn lengths_of_64_and_more_take_a_length_integer() {
    let base = vec![7u8; 500];
    let d = delta(&[window((0, 500), 300, &ins(0, 300, Some(100)), b"")]);
    assert_eq!(apply_ok(&d, &base), vec![7u8; 300]);
    let long = vec![9u8; 200];
    let d = delta(&[window((0, 0), 200, &ins(2, 200, None), &long)]);
    assert_eq!(apply_ok(&d, b""), long);
}

#[test]
fn several_windows_concatenate_and_each_has_its_own_source_view() {
    let base = b"AAAABBBBCCCC";
    let d = delta(&[
        window((0, 4), 4, &ins(0, 4, Some(0)), b""),
        window(
            (8, 4),
            6,
            &{
                let mut i = ins(0, 4, Some(0));
                i.extend(ins(2, 2, None));
                i
            },
            b"!!",
        ),
        window((4, 4), 4, &ins(0, 4, Some(0)), b""),
    ]);
    assert_eq!(apply_ok(&d, base), b"AAAACCCC!!BBBB");
}

#[test]
fn an_empty_target_is_the_header_alone_or_a_window_of_nothing() {
    assert_eq!(apply_ok(HEADER, b"anything"), b"");
    assert_eq!(apply_ok(&delta(&[window((0, 0), 0, b"", b"")]), b""), b"");
}

#[test]
fn a_target_copy_is_relative_to_its_own_window() {
    // window 2's target copy offset 0 is window 2's first byte, not window 1's
    let d = delta(&[
        window((0, 0), 2, &ins(2, 2, None), b"xy"),
        window(
            (0, 0),
            4,
            &{
                let mut i = ins(2, 2, None);
                i.extend(ins(1, 2, Some(0)));
                i
            },
            b"pq",
        ),
    ]);
    assert_eq!(apply_ok(&d, b""), b"xypqpq");
}

// ---- the header ----------------------------------------------------------------------------------------

#[test]
fn versions_1_and_2_are_unsupported_by_number_and_others_are_malformed() {
    assert_eq!(
        check_header(b"SVN\x01").unwrap_err(),
        DiffError::UnsupportedVersion(1)
    );
    assert_eq!(
        check_header(b"SVN\x02rest").unwrap_err(),
        DiffError::UnsupportedVersion(2)
    );
    assert!(matches!(
        check_header(b"SVN\x03"),
        Err(DiffError::Malformed(_))
    ));
    assert!(matches!(
        check_header(b"SVN\xff"),
        Err(DiffError::Malformed(_))
    ));
    assert!(matches!(
        check_header(b"SVM\x00"),
        Err(DiffError::Malformed(_))
    ));
    assert!(matches!(check_header(b"SVN"), Err(DiffError::Malformed(_))));
    assert!(matches!(check_header(b""), Err(DiffError::Malformed(_))));
    assert!(check_header(b"SVN\x00").is_ok());
    assert_eq!(
        apply(b"SVN\x01", b"", BIG).unwrap_err(),
        DiffError::UnsupportedVersion(1)
    );
}

// ---- every malformed case gives its typed error --------------------------------------------------------

#[test]
fn a_source_view_outside_the_base_is_malformed() {
    let d = delta(&[window((5, 10), 0, b"", b"")]);
    assert!(malformed(&d, b"short").contains("source view"));
    // an offset that overflows when added to the length
    let d = delta(&[window((u64::MAX, 2), 0, b"", b"")]);
    assert!(matches!(apply(&d, b"", BIG), Err(DiffError::Malformed(_))));
}

#[test]
fn a_source_copy_outside_the_source_view_is_malformed() {
    let d = delta(&[window((0, 4), 3, &ins(0, 3, Some(3)), b"")]);
    assert!(malformed(&d, b"abcdefgh").contains("outside the source view"));
    // the view itself is fine, the copy reaches past it (not past the base)
    let d = delta(&[window((0, 4), 6, &ins(0, 6, Some(0)), b"")]);
    assert!(malformed(&d, b"abcdefgh").contains("outside the source view"));
}

#[test]
fn a_target_copy_at_or_after_the_current_position_is_malformed() {
    // nothing written yet: offset 0 is the current position
    let d = delta(&[window((0, 0), 3, &ins(1, 3, Some(0)), b"")]);
    assert!(malformed(&d, b"").contains("current position"));
    let mut i = ins(2, 2, None);
    i.extend(ins(1, 2, Some(2)));
    let d = delta(&[window((0, 0), 4, &i, b"ab")]);
    assert!(malformed(&d, b"").contains("current position"));
}

#[test]
fn instructions_that_do_not_fill_the_target_view_are_malformed() {
    let d = delta(&[window((0, 0), 5, &ins(2, 3, None), b"abc")]);
    assert!(malformed(&d, b"").contains("do not fill"));
    let d = delta(&[window((0, 0), 2, &ins(2, 3, None), b"abc")]);
    assert!(malformed(&d, b"").contains("run past the target view"));
}

#[test]
fn unused_new_data_is_malformed() {
    let d = delta(&[window((0, 0), 2, &ins(2, 2, None), b"abcd")]);
    assert!(malformed(&d, b"").contains("new-data section"));
}

#[test]
fn a_new_data_instruction_that_needs_more_than_the_section_holds_is_malformed() {
    let d = delta(&[window((0, 0), 4, &ins(2, 4, None), b"ab")]);
    assert!(malformed(&d, b"").contains("truncated"));
}

#[test]
fn opcode_11_is_invalid() {
    let d = delta(&[window((0, 0), 1, &[0b1100_0001], b"")]);
    assert!(malformed(&d, b"").contains("opcode 11"));
}

#[test]
fn truncation_anywhere_is_malformed() {
    let full = delta(&[window(
        (0, 4),
        6,
        &{
            let mut i = ins(0, 4, Some(0));
            i.extend(ins(2, 2, None));
            i
        },
        b"!!",
    )]);
    assert_eq!(apply_ok(&full, b"abcd"), b"abcd!!");
    for cut in HEADER.len() + 1..full.len() {
        assert!(
            matches!(
                apply(&full[..cut], b"abcd", BIG),
                Err(DiffError::Malformed(_))
            ),
            "cut at {cut}"
        );
    }
}

#[test]
fn a_window_declaring_more_instruction_bytes_than_remain_is_malformed_without_allocating() {
    let mut w = Vec::new();
    for n in [0u64, 0, 10, u64::from(u32::MAX), 0] {
        w.extend(varint(n));
    }
    assert!(matches!(
        apply(&delta(&[w]), b"", BIG),
        Err(DiffError::Malformed(_))
    ));
}

#[test]
fn an_integer_that_does_not_fit_64_bits_is_malformed() {
    let mut d = HEADER.to_vec();
    d.extend([0xff; 11]);
    d.push(0x01);
    assert!(matches!(apply(&d, b"", BIG), Err(DiffError::Malformed(_))));
    // ten bytes whose value overflows u64
    let mut d = HEADER.to_vec();
    d.extend([0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f]);
    assert!(matches!(apply(&d, b"", BIG), Err(DiffError::Malformed(_))));
}

// ---- the ceiling ---------------------------------------------------------------------------------------

#[test]
fn a_target_over_the_ceiling_is_refused_before_its_instructions_run() {
    // Declares a 1 GiB target with an instruction stream that would need real work; refused up front.
    let d = delta(&[window((0, 0), 1 << 30, &ins(2, 1, None), b"a")]);
    assert_eq!(
        apply(&d, b"", 1000).unwrap_err(),
        DiffError::TooLarge { limit: 1000 }
    );
    // the ceiling is on the total across windows
    let d = delta(&[
        window((0, 0), 600, &ins(2, 600, None), &[b'x'; 600]),
        window((0, 0), 600, &ins(2, 600, None), &[b'y'; 600]),
    ]);
    assert_eq!(
        apply(&d, b"", 1000).unwrap_err(),
        DiffError::TooLarge { limit: 1000 }
    );
    assert_eq!(apply(&d, b"", 1200).unwrap().len(), 1200);
    // a run that fits is produced, one that would exceed the declared target is refused
    let mut i = ins(2, 1, None);
    i.extend(ins(1, 4096, Some(0)));
    let d = delta(&[window((0, 0), 4097, &i, b"r")]);
    assert_eq!(apply(&d, b"", 4097).unwrap().len(), 4097);
    assert_eq!(
        apply(&d, b"", 4096).unwrap_err(),
        DiffError::TooLarge { limit: 4096 }
    );
}

// ---- a seeded property test ----------------------------------------------------------------------------

/// A small deterministic generator (xorshift64*), so the test is reproducible without a dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

/// Random bytes after a valid header, against random bases, give only `Ok` or a typed error (never a panic),
/// and an `Ok` result never exceeds the ceiling. Mixed with structured windows whose fields are perturbed, so
/// the interesting paths (copies, runs) are reached and not only "truncated".
#[test]
fn random_deltas_give_only_typed_results_and_never_grow_past_the_ceiling() {
    let mut rng = Rng(0x5eed_1234_abcd_ef01);
    let ceiling = 4096usize;
    let (mut ok, mut err) = (0, 0);
    for case in 0..20_000 {
        let base_len = rng.below(64) as usize;
        let base = rng.bytes(base_len);
        let mut d = HEADER.to_vec();
        if case % 2 == 0 {
            // pure noise
            let n = rng.below(200) as usize;
            d.extend(rng.bytes(n));
        } else {
            // a window with a sane-ish shape whose numbers are random and small
            let mut instr = Vec::new();
            for _ in 0..rng.below(6) {
                let op = rng.below(4) as u8;
                let len = rng.below(70) as usize;
                let off = if op < 2 { Some(rng.below(80)) } else { None };
                instr.extend(ins(op, len, off));
            }
            let new_len = rng.below(40) as usize;
            let new = rng.bytes(new_len);
            d.extend(window(
                (rng.below(8), rng.below(70)),
                rng.below(120),
                &instr,
                &new,
            ));
        }
        match apply(&d, &base, ceiling) {
            Ok(t) => {
                assert!(t.len() <= ceiling, "case {case}: {} bytes", t.len());
                ok += 1;
            }
            Err(
                DiffError::Malformed(_)
                | DiffError::UnsupportedVersion(_)
                | DiffError::TooLarge { .. },
            ) => {
                err += 1;
            }
        }
    }
    assert!(
        ok > 0 && err > 0,
        "both outcomes were exercised ({ok} ok, {err} errors)"
    );
}
