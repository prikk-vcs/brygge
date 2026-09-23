//! Unit tests for the RCS `,v` reader: parsing, `@@` escaping, and reverse-delta reconstruction.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::{RevNum, parse_rcs};

/// A hand-written `,v` with three trunk revisions. 1.3 is full text; 1.2 and 1.1 are reverse deltas.
const SAMPLE: &[u8] = b"head\t1.3;\n\
access;\n\
symbols\n\
\tREL_1:1.2;\n\
locks; strict;\n\
comment\t@# @;\n\
\n\
\n\
1.3\n\
date\t2024.01.03.12.00.00;\tauthor alice;\tstate Exp;\n\
branches;\n\
next\t1.2;\n\
\n\
1.2\n\
date\t2024.01.02.12.00.00;\tauthor alice;\tstate Exp;\n\
branches;\n\
next\t1.1;\n\
\n\
1.1\n\
date\t2024.01.01.12.00.00;\tauthor bob;\tstate Exp;\n\
branches;\n\
next\t;\n\
\n\
\n\
desc\n\
@@\n\
\n\
\n\
1.3\n\
log\n\
@third@\n\
text\n\
@line one\nline two changed\nline three\n@\n\
\n\
\n\
1.2\n\
log\n\
@second@\n\
text\n\
@d2 1\na2 1\nline two\n@\n\
\n\
\n\
1.1\n\
log\n\
@first@\n\
text\n\
@d3 1\n@\n";

fn rev(s: &str) -> RevNum {
    RevNum::parse(s).unwrap()
}

#[test]
fn parses_admin_and_delta_metadata() {
    let f = parse_rcs(SAMPLE).unwrap();
    assert_eq!(f.head, rev("1.3"));
    assert_eq!(f.symbols, vec![("REL_1".to_string(), rev("1.2"))]);
    assert_eq!(f.revisions.len(), 3);
    let r13 = &f.revisions[&rev("1.3")];
    assert_eq!(r13.author, b"alice");
    assert_eq!(r13.state, "Exp");
    assert_eq!(r13.next.as_ref().unwrap(), &rev("1.2"));
    // 2024-01-03T12:00:00Z.
    assert_eq!(r13.date, 1_704_283_200);
    assert_eq!(f.revisions[&rev("1.1")].author, b"bob");
}

#[test]
fn reconstructs_each_revision_via_reverse_deltas() {
    let f = parse_rcs(SAMPLE).unwrap();
    assert_eq!(
        f.content_of(&rev("1.3")).unwrap(),
        b"line one\nline two changed\nline three\n"
    );
    assert_eq!(
        f.content_of(&rev("1.2")).unwrap(),
        b"line one\nline two\nline three\n"
    );
    assert_eq!(f.content_of(&rev("1.1")).unwrap(), b"line one\nline two\n");
}

#[test]
fn unescapes_doubled_at_signs_in_strings() {
    // A log containing a literal @ is stored doubled (@@).
    let vfile = b"head\t1.1;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.1\ndate\t2024.01.01.00.00.00;\tauthor a;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.1\nlog\n@see a@@b for detail@\ntext\n@hi\n@\n";
    let f = parse_rcs(vfile).unwrap();
    assert_eq!(f.revisions[&rev("1.1")].log, b"see a@b for detail");
    assert_eq!(f.content_of(&rev("1.1")).unwrap(), b"hi\n");
}

#[test]
fn a_dead_revision_is_read() {
    // head 1.2 is the (dead) newest and stores full text; 1.1 is a reverse delta (here an identity/no-op).
    let vfile = b"head\t1.2;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.2\ndate\t2024.01.02.00.00.00;\tauthor a;\tstate dead;\nbranches;\nnext\t1.1;\n\n\
1.1\ndate\t2024.01.01.00.00.00;\tauthor a;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.2\nlog\n@remove@\ntext\n@only line\n@\n\n\n\
1.1\nlog\n@add@\ntext\n@@\n";
    let f = parse_rcs(vfile).unwrap();
    assert_eq!(f.revisions[&rev("1.2")].state, "dead");
    assert_eq!(f.content_of(&rev("1.1")).unwrap(), b"only line\n");
}

#[test]
fn a_truncated_at_string_is_a_typed_error_not_a_panic() {
    let vfile = b"head\t1.1;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.1\ndate\t2024.01.01.00.00.00;\tauthor a;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.1\nlog\n@unterminated";
    assert!(parse_rcs(vfile).is_err());
}

#[test]
fn an_unparseable_date_is_a_read_error_not_a_fabricated_epoch() {
    // Review 008 R-3: a malformed date must never silently become `0` (1970) — that would fabricate a
    // false claim that then feeds clustering and `repo_id`.
    let vfile = b"head\t1.1;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.1\ndate\tnot-a-date;\tauthor a;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.1\nlog\n@x@\ntext\n@x@\n";
    let err = parse_rcs(vfile).unwrap_err();
    assert!(
        matches!(err, crate::Error::Read(_)),
        "expected Error::Read, got {err:?}"
    );
}

#[test]
fn a_year_outside_1970_9999_is_rejected() {
    // Review 008 R-3: bounded years — an absurd year must not become a valid-looking, wildly wrong
    // timestamp.
    let vfile = b"head\t1.1;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.1\ndate\t0001.01.01.00.00.00;\tauthor a;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.1\nlog\n@x@\ntext\n@x@\n";
    assert!(parse_rcs(vfile).is_err());
}

#[test]
fn a_malformed_branch_field_is_a_read_error_not_a_silent_unset() {
    // Review 008 R-5: a present-but-unparseable `branch` field must not silently become "no vendor
    // branch" — that would treat an inconsistent file's vendor revisions as ordinary branch revisions.
    let vfile = b"head\t1.1;\nbranch\tnot-a-revnum;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.1\ndate\t2024.01.01.00.00.00;\tauthor a;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.1\nlog\n@x@\ntext\n@x@\n";
    let err = parse_rcs(vfile).unwrap_err();
    assert!(
        matches!(err, crate::Error::Read(_)),
        "expected Error::Read, got {err:?}"
    );
}

#[test]
fn a_non_utf8_symbol_name_is_refused() {
    // Review 008 R-4: no lossy conversion anywhere identity- or reference-bearing — a symbol name that
    // is not valid UTF-8 cannot be carried as an IR ref name, so it is refused, not mangled.
    let mut vfile = b"head\t1.1;\naccess;\nsymbols\n\t".to_vec();
    vfile.extend_from_slice(&[0xff, 0xfe]); // invalid UTF-8
    vfile.extend_from_slice(
        b":1.1;\nlocks; strict;\n\n\n\
1.1\ndate\t2024.01.01.00.00.00;\tauthor a;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.1\nlog\n@x@\ntext\n@x@\n",
    );
    let err = parse_rcs(&vfile).unwrap_err();
    assert!(
        matches!(err, crate::Error::FloorRefusal { ref feature, .. } if feature == "non-utf8-symbol-name"),
        "expected a non-utf8-symbol-name FloorRefusal, got {err:?}"
    );
}

#[test]
fn author_is_carried_byte_exact_not_lossily_converted() {
    // Review 008 R-4: the author is carried as raw bytes — a non-UTF-8 login is preserved exactly, not
    // replaced with U+FFFD.
    let mut vfile = b"head\t1.1;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.1\ndate\t2024.01.01.00.00.00;\tauthor "
        .to_vec();
    vfile.extend_from_slice(&[0xff, 0xfe]); // an author login that is not valid UTF-8
    vfile.extend_from_slice(
        b";\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.1\nlog\n@x@\ntext\n@x@\n",
    );
    let f = parse_rcs(&vfile).unwrap();
    assert_eq!(f.revisions[&rev("1.1")].author, vec![0xff, 0xfe]);
}

// ---- review 008 §8 F-1: every date component bounded, arithmetic checked ---------------------------------

#[test]
fn a_valid_date_still_parses() {
    use super::parse_rcs_date;
    assert_eq!(parse_rcs_date("1970.01.01.00.00.00"), Some(0));
    assert_eq!(
        parse_rcs_date("2000.02.29.23.59.60"),
        Some(951_782_400 + 86_400)
    );
}

#[test]
fn each_date_component_is_bounded() {
    use super::parse_rcs_date;
    assert!(parse_rcs_date("2024.01.01.24.00.00").is_none(), "hour 24");
    assert!(parse_rcs_date("2024.01.01.-1.00.00").is_none(), "hour -1");
    assert!(parse_rcs_date("2024.01.01.00.60.00").is_none(), "minute 60");
    assert!(parse_rcs_date("2024.01.01.00.00.61").is_none(), "second 61");
    assert!(parse_rcs_date("2024.01.01.00.00.-1").is_none(), "second -1");
    assert!(parse_rcs_date("2024.13.01.00.00.00").is_none(), "month 13");
    assert!(parse_rcs_date("2024.00.01.00.00.00").is_none(), "month 0");
    assert!(parse_rcs_date("2024.01.00.00.00.00").is_none(), "day 0");
    assert!(parse_rcs_date("2024.01.32.00.00.00").is_none(), "day 32");
}

#[test]
fn a_day_beyond_the_months_length_is_rejected_with_leap_years_right() {
    use super::parse_rcs_date;
    assert!(
        parse_rcs_date("1999.02.29.00.00.00").is_none(),
        "1999 is not a leap year"
    );
    assert!(
        parse_rcs_date("2024.02.30.00.00.00").is_none(),
        "Feb 30 never exists"
    );
    assert!(
        parse_rcs_date("2024.04.31.00.00.00").is_none(),
        "April has 30 days"
    );
    assert!(
        parse_rcs_date("2024.02.29.00.00.00").is_some(),
        "2024 is a leap year"
    );
    assert!(
        parse_rcs_date("2000.02.29.00.00.00").is_some(),
        "2000 is a leap year (div by 400)"
    );
    assert!(
        parse_rcs_date("2100.02.29.00.00.00").is_none(),
        "2100 is not (div by 100, not 400)"
    );
}

#[test]
fn a_huge_component_cannot_wrap() {
    use super::parse_rcs_date;
    assert!(parse_rcs_date("2024.01.01.9223372036854775807.00.00").is_none());
    assert!(parse_rcs_date("2024.01.01.00.9223372036854775807.00").is_none());
    assert!(parse_rcs_date("2024.01.01.00.00.9223372036854775807").is_none());
}

// ---- F-2 / F-3 ----------------------------------------------------------------------------------------

#[test]
fn a_symbol_whose_revision_does_not_parse_is_a_read_error_not_skipped() {
    let vfile = b"head\t1.1;\naccess;\nsymbols\n\tTAG:not-a-rev;\nlocks; strict;\n\n\n\
1.1\ndate\t2024.01.01.00.00.00;\tauthor a;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.1\nlog\n@x@\ntext\n@x@\n";
    match parse_rcs(vfile).unwrap_err() {
        crate::Error::Read(m) => assert!(m.contains("malformed symbol revision"), "{m}"),
        other => panic!("expected Error::Read, got {other:?}"),
    }
}

#[test]
fn a_branch_field_must_be_a_branch_number() {
    // `branch 1;` and `branch 1.2;` parse as revision numbers but are not branch numbers (odd component
    // count >= 3), so they are malformed — not a default branch, and not a trunk-after-branch refusal.
    for bad in ["1", "1.2", "1.1.1.1"] {
        let vfile = format!(
            "head\t1.1;\nbranch\t{bad};\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.1\ndate\t2024.01.01.00.00.00;\tauthor a;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.1\nlog\n@x@\ntext\n@x@\n"
        );
        assert!(
            matches!(parse_rcs(vfile.as_bytes()), Err(crate::Error::Read(_))),
            "branch {bad} must be a Read error"
        );
    }
}

#[test]
fn a_well_formed_vendor_branch_field_still_parses() {
    let vfile = b"head\t1.1;\nbranch\t1.1.1;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.1\ndate\t2024.01.01.00.00.00;\tauthor a;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.1\nlog\n@x@\ntext\n@x@\n";
    assert_eq!(parse_rcs(vfile).unwrap().branch, RevNum::parse("1.1.1"));
}
