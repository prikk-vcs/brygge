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
    assert_eq!(r13.author, "alice");
    assert_eq!(r13.state, "Exp");
    assert_eq!(r13.next.as_ref().unwrap(), &rev("1.2"));
    // 2024-01-03T12:00:00Z.
    assert_eq!(r13.date, 1_704_283_200);
    assert_eq!(f.revisions[&rev("1.1")].author, "bob");
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
