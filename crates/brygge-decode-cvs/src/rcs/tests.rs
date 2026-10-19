//! Unit tests for the RCS `,v` reader: parsing, `@@` escaping, and reverse-delta reconstruction.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::{RevNum, parse_rcs};
use crate::Error;

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
fn an_ed_add_commands_claimed_count_does_not_preallocate_past_the_script() {
    // RFC 014 D-1 mutation finding: an `a<n> <count>` ed command's `count` used to pre-allocate a
    // `Vec::with_capacity(count)` directly — a decompression-bomb-shaped bug (RUSTSEC-2026-0344's class):
    // a claimed count of billions, with no lines actually following it, would try to reserve gigabytes
    // before the loop's own bounds check ever ran. It must now be a typed error, fast, not an allocation
    // attempt sized by an attacker-chosen field.
    // head (1.2) carries full text; 1.1's stored `text` is the reverse delta applied to reach it, which
    // is where an ed command's `count` is read — so the attack must land on a non-head revision.
    let vfile = b"head\t1.2;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.2\ndate\t2024.01.02.00.00.00;\tauthor a;\tstate Exp;\nbranches;\nnext\t1.1;\n\n\
1.1\ndate\t2024.01.01.00.00.00;\tauthor a;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.2\nlog\n@head@\ntext\n@only line\n@\n\n\n\
1.1\nlog\n@x@\ntext\n@a1 999999999999\nonly one line follows\n@\n";
    let f = parse_rcs(vfile).unwrap();
    assert!(f.content_of(&rev("1.1")).is_err());
}

#[test]
fn an_ed_commands_claimed_line_number_cannot_spin_the_apply_loop() {
    // RFC 014 D-1 mutation finding: `d<n> <count>`/`a<n> <count>`'s `n` drove the copy-unchanged-lines
    // loop directly, with no bound tied to the source's actual (already-bounded) length — a single
    // `d99999999999 1` command on a two-line source hung the decoder for minutes, confirmed by hand before
    // the fix. It must now resolve near-instantly however large the claimed line number is.
    let vfile = b"head\t1.2;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.2\ndate\t2024.01.02.00.00.00;\tauthor a;\tstate Exp;\nbranches;\nnext\t1.1;\n\n\
1.1\ndate\t2024.01.01.00.00.00;\tauthor a;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.2\nlog\n@head@\ntext\n@only line\n@\n\n\n\
1.1\nlog\n@x@\ntext\n@d99999999999 1\n@\n";
    let f = parse_rcs(vfile).unwrap();
    let start = std::time::Instant::now();
    let _ = f.content_of(&rev("1.1"));
    assert!(
        start.elapsed() < std::time::Duration::from_secs(1),
        "took {:?}, far longer than a bounded-size source should ever need",
        start.elapsed()
    );
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

// ---- RFC 010 increment 3: every revision's content in one pass ---------------------------------------------

const LINES: usize = 12;

fn base_lines() -> Vec<String> {
    (0..LINES).map(|l| format!("line {l} base\n")).collect()
}

/// `text` with the lines at `at` (mod LINES) rewritten to carry `tag`.
fn edited(text: &[String], at: &[usize], tag: &str) -> Vec<String> {
    let mut out = text.to_vec();
    for &a in at {
        let l = a % LINES;
        out[l] = format!("line {l} {tag}\n");
    }
    out
}

/// The ed-style RCS diff that turns `from` into `to` (same number of lines: one replace per changed line).
fn delta(from: &[String], to: &[String]) -> String {
    let mut d = String::new();
    for (i, (a, b)) in from.iter().zip(to).enumerate() {
        if a != b {
            d.push_str(&format!("d{} 1\na{} 1\n{b}", i + 1, i + 1));
        }
    }
    d
}

fn joined(lines: &[String]) -> Vec<u8> {
    lines.concat().into_bytes()
}

/// A `,v` with `n` trunk revisions (1.n is full text, older ones reverse deltas) and a branch of `blen`
/// revisions off 1.`bp` (`1.bp.2.1` ...: forward deltas). Returns the file and the intended text of every
/// revision.
fn long_trunk_with_a_branch(n: usize, bp: usize, blen: usize) -> (Vec<u8>, Vec<(String, Vec<u8>)>) {
    let mut trunk: Vec<Vec<String>> = vec![Vec::new(), base_lines()]; // 1-based
    for k in 2..=n {
        let prev = trunk[k - 1].clone();
        trunk.push(edited(&prev, &[k * 3, k * 5 + 1], &format!("r{k}")));
    }
    let mut branch: Vec<Vec<String>> = Vec::new();
    let mut prev = trunk[bp].clone();
    for j in 1..=blen {
        let next = edited(&prev, &[j * 7], &format!("b{j}"));
        branch.push(next.clone());
        prev = next;
    }
    let bnum = |j: usize| format!("1.{bp}.2.{j}");

    let mut s = format!("head\t1.{n};\naccess;\nsymbols;\nlocks; strict;\n\n\n");
    for k in (1..=n).rev() {
        let next = if k > 1 {
            format!("1.{}", k - 1)
        } else {
            String::new()
        };
        let branches = if k == bp && blen > 0 {
            format!("branches\n\t{};\n", bnum(1))
        } else {
            "branches;\n".to_string()
        };
        s.push_str(&format!(
            "1.{k}\ndate\t2024.01.{:02}.00.00.00;\tauthor a;\tstate Exp;\n{branches}next\t{next};\n\n",
            k.min(28)
        ));
        if k == bp {
            for j in 1..=blen {
                let next = if j < blen { bnum(j + 1) } else { String::new() };
                s.push_str(&format!(
                    "{}\ndate\t2024.02.{:02}.00.00.00;\tauthor a;\tstate Exp;\nbranches;\nnext\t{next};\n\n",
                    bnum(j),
                    j.min(28)
                ));
            }
        }
    }
    s.push_str("\ndesc\n@@\n\n\n");
    s.push_str(&format!(
        "1.{n}\nlog\n@r{n}@\ntext\n@{}@\n",
        trunk[n].concat()
    ));
    for k in (1..n).rev() {
        s.push_str(&format!(
            "\n1.{k}\nlog\n@r{k}@\ntext\n@{}@\n",
            delta(&trunk[k + 1], &trunk[k])
        ));
        if k == bp {
            let mut from = trunk[bp].clone();
            for j in 1..=blen {
                s.push_str(&format!(
                    "\n{}\nlog\n@b{j}@\ntext\n@{}@\n",
                    bnum(j),
                    delta(&from, &branch[j - 1])
                ));
                from = branch[j - 1].clone();
            }
        }
    }
    let mut want: Vec<(String, Vec<u8>)> = (1..=n)
        .map(|k| (format!("1.{k}"), joined(&trunk[k])))
        .collect();
    want.extend((1..=blen).map(|j| (bnum(j), joined(&branch[j - 1]))));
    (s.into_bytes(), want)
}

fn num(s: &str) -> RevNum {
    RevNum(s.split('.').map(|c| c.parse().unwrap()).collect())
}

#[test]
fn one_pass_contents_equal_per_revision_contents_for_every_revision() {
    let (v, want) = long_trunk_with_a_branch(40, 12, 5);
    let f = parse_rcs(&v).unwrap();
    let all: std::collections::BTreeSet<RevNum> = want.iter().map(|(r, _)| num(r)).collect();
    assert_eq!(all.len(), 45, "40 trunk revisions and 5 branch revisions");

    let many = f.contents_of_many(&all).unwrap();
    assert_eq!(many.len(), all.len());
    for (rev, intended) in &want {
        let n = num(rev);
        assert_eq!(
            many[&n],
            f.content_of(&n).unwrap(),
            "{rev}: the pass equals content_of"
        );
        assert_eq!(
            &many[&n], intended,
            "{rev}: and both equal the generator's text"
        );
    }
}

#[test]
fn one_pass_yields_only_what_is_asked_for_and_stops_early() {
    let (v, want) = long_trunk_with_a_branch(40, 12, 5);
    let f = parse_rcs(&v).unwrap();
    // A few trunk revisions, one of them the head, and the branch's last revision (needs 1.12 too).
    let asked: std::collections::BTreeSet<RevNum> = ["1.40", "1.31", "1.13", "1.12.2.5"]
        .iter()
        .map(|r| num(r))
        .collect();
    let many = f.contents_of_many(&asked).unwrap();
    assert_eq!(
        many.len(),
        4,
        "exactly the revisions asked for, not the branch point they pass"
    );
    for (rev, intended) in want.iter().filter(|(r, _)| asked.contains(&num(r))) {
        assert_eq!(&many[&num(rev)], intended, "{rev}");
    }
    assert!(
        f.contents_of_many(&std::collections::BTreeSet::new())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_revision_that_is_not_reachable_is_an_error_not_a_silent_omission() {
    let (v, _) = long_trunk_with_a_branch(6, 3, 2);
    let f = parse_rcs(&v).unwrap();
    for missing in ["1.9", "1.3.2.9", "1.5.2.1"] {
        let asked = std::collections::BTreeSet::from([num("1.6"), num(missing)]);
        assert!(
            f.contents_of_many(&asked).is_err(),
            "{missing} is not in this file"
        );
        assert!(
            f.content_of(&num(missing)).is_err(),
            "and content_of agrees"
        );
    }
}

// ---- RFC 013 §6: nested branches in the one pass ---------------------------------------------------------

/// One branch of a generated file: cut from revision `point` (trunk or a branch revision), numbered `no`
/// (`point.no.1` ...), with `len` revisions.
struct BranchSpec {
    point: &'static str,
    no: u32,
    len: usize,
}

/// A `,v` with `n` trunk revisions and any number of branches, nested as far as the specs say (a spec may
/// name a branch revision of an earlier spec as its `point`). Returns the file and the intended text of every
/// revision.
fn tree_file(n: usize, specs: &[BranchSpec]) -> (Vec<u8>, Vec<(String, Vec<u8>)>) {
    use std::collections::BTreeMap;
    let mut text: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut prev = base_lines();
    text.insert("1.1".to_string(), prev.clone());
    for k in 2..=n {
        prev = edited(&prev, &[k * 3, k * 5 + 1], &format!("r{k}"));
        text.insert(format!("1.{k}"), prev.clone());
    }
    // (revision, its predecessor on its own line, whether the predecessor is the branch point)
    let mut branch_revs: Vec<(String, String)> = Vec::new();
    for spec in specs {
        let mut from = text[spec.point].clone();
        let mut from_name = spec.point.to_string();
        for j in 1..=spec.len {
            let name = format!("{}.{}.{j}", spec.point, spec.no);
            let next = edited(
                &from,
                &[j * 7 + spec.no as usize],
                &format!("{}b{j}", spec.no),
            );
            text.insert(name.clone(), next.clone());
            branch_revs.push((name.clone(), from_name.clone()));
            from = next;
            from_name = name;
        }
    }
    let first_of = |point: &str| -> Vec<String> {
        specs
            .iter()
            .filter(|b| b.point == point && b.len > 0)
            .map(|b| format!("{point}.{}.1", b.no))
            .collect()
    };
    let admin = |name: &str, next: Option<String>| {
        let branches = first_of(name);
        let brs = if branches.is_empty() {
            "branches;\n".to_string()
        } else {
            format!("branches\n\t{};\n", branches.join("\n\t"))
        };
        format!(
            "{name}\ndate\t2024.01.01.00.00.00;\tauthor a;\tstate Exp;\n{brs}next\t{};\n\n",
            next.unwrap_or_default()
        )
    };
    let mut s = format!("head\t1.{n};\naccess;\nsymbols;\nlocks; strict;\n\n\n");
    for k in (1..=n).rev() {
        s.push_str(&admin(
            &format!("1.{k}"),
            (k > 1).then(|| format!("1.{}", k - 1)),
        ));
    }
    for spec in specs {
        for j in 1..=spec.len {
            let name = format!("{}.{}.{j}", spec.point, spec.no);
            let next = (j < spec.len).then(|| format!("{}.{}.{}", spec.point, spec.no, j + 1));
            s.push_str(&admin(&name, next));
        }
    }
    s.push_str("\ndesc\n@@\n\n\n");
    s.push_str(&format!(
        "1.{n}\nlog\n@r{n}@\ntext\n@{}@\n",
        text[&format!("1.{n}")].concat()
    ));
    for k in (1..n).rev() {
        s.push_str(&format!(
            "\n1.{k}\nlog\n@r{k}@\ntext\n@{}@\n",
            delta(&text[&format!("1.{}", k + 1)], &text[&format!("1.{k}")])
        ));
    }
    for (name, from) in &branch_revs {
        s.push_str(&format!(
            "\n{name}\nlog\n@b@\ntext\n@{}@\n",
            delta(&text[from], &text[name])
        ));
    }
    let want = text.into_iter().map(|(k, v)| (k, joined(&v))).collect();
    (s.into_bytes(), want)
}

fn every_pass_equals_content_of(n: usize, specs: &[BranchSpec]) {
    let (v, want) = tree_file(n, specs);
    let f = parse_rcs(&v).unwrap();
    let all: std::collections::BTreeSet<RevNum> = want.iter().map(|(r, _)| num(r)).collect();
    let many = f.contents_of_many(&all).unwrap();
    assert_eq!(many.len(), all.len());
    for (rev, intended) in &want {
        let n = num(rev);
        assert_eq!(
            many[&n],
            f.content_of(&n).unwrap(),
            "{rev}: the pass equals content_of"
        );
        assert_eq!(
            &many[&n], intended,
            "{rev}: and both equal the generator's text"
        );
    }
}

/// Handoff test 12: nested branches, two and three levels deep, siblings, and a branch cut from the first
/// revision of a branch: the one pass equals `content_of` for every revision.
#[test]
fn one_pass_equals_content_of_for_nested_branches() {
    every_pass_equals_content_of(
        12,
        &[
            BranchSpec {
                point: "1.5",
                no: 2,
                len: 4,
            },
            BranchSpec {
                point: "1.5",
                no: 4,
                len: 2,
            },
            BranchSpec {
                point: "1.5.2.2",
                no: 2,
                len: 3,
            },
            BranchSpec {
                point: "1.5.2.2.2.3",
                no: 2,
                len: 2,
            },
            BranchSpec {
                point: "1.5.2.1",
                no: 2,
                len: 2,
            },
            BranchSpec {
                point: "1.9",
                no: 2,
                len: 1,
            },
            BranchSpec {
                point: "1.1",
                no: 2,
                len: 2,
            },
        ],
    );
}

/// The shape a `cvs import` then `cvs tag -b` leaves: the vendor branch `1.1.1` off `1.1`, and a branch cut
/// from the vendor revision `1.1.1.1` (`1.1.1.1.2.1`).
#[test]
fn one_pass_equals_content_of_for_a_branch_cut_from_a_vendor_revision() {
    every_pass_equals_content_of(
        3,
        &[
            BranchSpec {
                point: "1.1",
                no: 1,
                len: 2,
            },
            BranchSpec {
                point: "1.1.1.1",
                no: 2,
                len: 3,
            },
            BranchSpec {
                point: "1.2",
                no: 2,
                len: 1,
            },
        ],
    );
}

/// Asking only for a deep revision still walks every line above it, once, and returns just what was asked.
#[test]
fn a_deep_revision_alone_pulls_in_its_branch_points_and_returns_only_itself() {
    let specs = [
        BranchSpec {
            point: "1.4",
            no: 2,
            len: 3,
        },
        BranchSpec {
            point: "1.4.2.3",
            no: 2,
            len: 2,
        },
        BranchSpec {
            point: "1.4.2.3.2.2",
            no: 2,
            len: 2,
        },
    ];
    let (v, want) = tree_file(8, &specs);
    let f = parse_rcs(&v).unwrap();
    let asked = std::collections::BTreeSet::from([num("1.4.2.3.2.2.2.2")]);
    let many = f.contents_of_many(&asked).unwrap();
    assert_eq!(many.len(), 1);
    let (_, intended) = want.iter().find(|(r, _)| r == "1.4.2.3.2.2.2.2").unwrap();
    assert_eq!(&many[&num("1.4.2.3.2.2.2.2")], intended);
}

#[test]
fn a_nested_branch_point_that_does_not_exist_is_an_error_not_a_silent_omission() {
    let (v, _) = tree_file(
        4,
        &[BranchSpec {
            point: "1.2",
            no: 2,
            len: 2,
        }],
    );
    let f = parse_rcs(&v).unwrap();
    for missing in ["1.2.2.9.2.1", "1.2.4.1.2.1"] {
        let asked = std::collections::BTreeSet::from([num(missing)]);
        assert!(f.contents_of_many(&asked).is_err(), "{missing}");
        assert!(
            f.content_of(&num(missing)).is_err(),
            "{missing}: content_of agrees"
        );
    }
}

#[test]
fn a_truncated_symbols_list_is_a_read_error_not_an_infinite_loop() {
    // RFC 014 D-1 mutation finding: truncating a file right after "symbols\n" (no closing `;`) spun the
    // admin-section parser forever — `cur.pos` kept advancing past the end of the buffer with no
    // termination condition. Found by the mutation harness's truncate operator on the very first seed.
    let vfile = b"head\t1.3;\naccess;\nsymbols\n";
    let start = std::time::Instant::now();
    let result = parse_rcs(vfile);
    assert!(
        start.elapsed() < std::time::Duration::from_secs(1),
        "took {:?}: looks like it is spinning again",
        start.elapsed()
    );
    assert!(result.is_err());
}

#[test]
fn a_truncated_branches_list_is_a_read_error_not_an_infinite_loop() {
    // Same bug, same fix, the delta section's `branches` field.
    let vfile = b"head\t1.1;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.1\ndate\t2024.01.01.00.00.00;\tauthor a;\tstate Exp;\nbranches\n";
    let start = std::time::Instant::now();
    let result = parse_rcs(vfile);
    assert!(
        start.elapsed() < std::time::Duration::from_secs(1),
        "took {:?}: looks like it is spinning again",
        start.elapsed()
    );
    assert!(result.is_err());
}

#[test]
fn a_malformed_revision_number_shape_is_a_read_error_not_a_subtraction_panic() {
    // RFC 014 D-1 mutation finding: `delta_path`'s branch arithmetic (`n - 2`, `n - 1`) assumed every
    // non-trunk revision number has at least 3 components; a mutated `head` of `1.33.12.00.00` (5
    // components — a shape neither trunk nor an actual reachable branch revision) made it subtract past
    // zero and panic with overflow checks on. Found by the mutation harness via a boundary write plus a
    // splice, within a few thousand iterations of the very first seed.
    let vfile = b"head\t1.33.12.00.00;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.33.12.00.00\ndate\t2024.01.01.00.00.00;\tauthor a;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.33.12.00.00\nlog\n@x@\ntext\n@only line\n@\n";
    let f = parse_rcs(vfile).unwrap();
    assert!(f.content_of(&f.head).is_err());
}

#[test]
fn a_next_cycle_is_refused_in_a_bounded_number_of_steps_not_the_full_max_chain() {
    // Review 059 F-3: a 2-revision cycle in `next` (head and 1.2 point at each other; 1.1 exists but is
    // never reached) used to cost the full MAX_CHAIN (1,000,000 steps, ~40ms measured by hand) before the
    // existing "chain too long" refusal. Bounding the walk by the file's own revision count instead turns
    // this into a handful of steps: a loose 50ms ceiling (two orders of magnitude above what a few
    // revisions should ever need) still catches a regression back to the unbounded walk, without being
    // so tight that ordinary machine noise could fail this test.
    let vfile = b"head\t1.3;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.3\ndate\t2024.01.03.00.00.00;\tauthor a;\tstate Exp;\nbranches;\nnext\t1.2;\n\n\
1.2\ndate\t2024.01.02.00.00.00;\tauthor a;\tstate Exp;\nbranches;\nnext\t1.3;\n\n\
1.1\ndate\t2024.01.01.00.00.00;\tauthor a;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.3\nlog\n@x@\ntext\n@only line\n@\n\n\n\
1.2\nlog\n@y@\ntext\n@only line\n@\n\n\n\
1.1\nlog\n@z@\ntext\n@only line\n@\n";
    let f = parse_rcs(vfile).unwrap();
    let start = std::time::Instant::now();
    let result = f.content_of(&rev("1.1"));
    let elapsed = start.elapsed();
    assert!(result.is_err());
    assert!(
        elapsed < std::time::Duration::from_millis(50),
        "took {elapsed:?}: looks like it ran the full MAX_CHAIN again"
    );
}

// ---- RFC 014 edit-script strictness (review 063's ruling) -------------------------------------------
//
// head (1.2) is the two-line fulltext "line one\n"/"line two\n"; 1.1's stored text is the ed script
// under test, applied in reverse to reach 1.1 from 1.2. `consumed` starts at 0.

fn two_line_source_with_delta(diff: &str) -> Vec<u8> {
    let mut v = b"head\t1.2;\naccess;\nsymbols;\nlocks; strict;\n\n\n\
1.2\ndate\t2024.01.02.00.00.00;\tauthor a;\tstate Exp;\nbranches;\nnext\t1.1;\n\n\
1.1\ndate\t2024.01.01.00.00.00;\tauthor a;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.2\nlog\n@head@\ntext\n@line one\nline two\n@\n\n\n\
1.1\nlog\n@delta@\ntext\n@"
        .to_vec();
    v.extend(diff.as_bytes());
    v.extend(b"@\n");
    v
}

fn content_of_1_1(diff: &str) -> Result<Vec<u8>, Error> {
    let vfile = two_line_source_with_delta(diff);
    let f = parse_rcs(&vfile).unwrap();
    f.content_of(&rev("1.1"))
}

#[test]
fn an_out_of_order_delete_is_refused_naming_the_command() {
    let e = content_of_1_1("d2 1\nd1 1\n").unwrap_err();
    let Error::Read(msg) = e else {
        panic!("expected Read, got {e:?}")
    };
    assert!(msg.contains("d1 1"), "{msg}");
    assert!(msg.contains("out of order"), "{msg}");
}

#[test]
fn an_overlapping_delete_is_refused() {
    let e = content_of_1_1("d1 2\nd1 1\n").unwrap_err();
    assert!(matches!(e, Error::Read(_)));
}

#[test]
fn a_delete_past_the_end_is_refused() {
    let e = content_of_1_1("d1 5\n").unwrap_err();
    let Error::Read(msg) = e else {
        panic!("expected Read, got {e:?}")
    };
    assert!(msg.contains("d1 5"), "{msg}");
    assert!(msg.contains("past the end"), "{msg}");
}

#[test]
fn an_add_past_the_end_is_refused() {
    let e = content_of_1_1("a9 1\nnew\n").unwrap_err();
    let Error::Read(msg) = e else {
        panic!("expected Read, got {e:?}")
    };
    assert!(msg.contains("a9 1"), "{msg}");
    assert!(msg.contains("past the end"), "{msg}");
}

#[test]
fn an_out_of_order_add_is_refused() {
    let e = content_of_1_1("a2 1\nx\nd1 1\n").unwrap_err();
    assert!(matches!(e, Error::Read(_)));
}

#[test]
fn a_zero_count_add_is_refused_by_name_regardless_of_position() {
    // Review 063 §3: CVS's own apply_rcs_changes writes one line anyway for a zero-count add — past the
    // end of its allocated vector. Undefined behaviour, not a well-defined no-op; brygge refuses it
    // unconditionally rather than matching CVS's order-dependent result.
    let e = content_of_1_1("a1 0\n").unwrap_err();
    let Error::Read(msg) = e else {
        panic!("expected Read, got {e:?}")
    };
    assert!(msg.contains("a1 0"), "{msg}");
    assert!(msg.contains("zero-count add"), "{msg}");
}

#[test]
fn an_add_right_after_a_delete_ending_at_the_same_line_is_valid() {
    assert_eq!(content_of_1_1("d1 2\na2 1\nnew\n").unwrap(), b"new\n");
}

#[test]
fn a_delete_of_the_last_line_is_valid() {
    assert_eq!(content_of_1_1("d2 1\n").unwrap(), b"line one\n");
}

#[test]
fn an_add_after_the_last_line_is_valid() {
    assert_eq!(
        content_of_1_1("a2 1\nextra\n").unwrap(),
        b"line one\nline two\nextra\n"
    );
}

#[test]
fn an_add_at_0_before_the_first_line_is_valid() {
    assert_eq!(
        content_of_1_1("a0 1\nfirst\n").unwrap(),
        b"first\nline one\nline two\n"
    );
}

#[test]
fn a_zero_count_delete_in_order_and_in_range_is_a_no_op() {
    assert_eq!(content_of_1_1("d1 0\n").unwrap(), b"line one\nline two\n");
    // Also valid right after a delete that consumed everything, exactly at the boundary.
    assert_eq!(content_of_1_1("d1 2\nd3 0\n").unwrap(), b"");
}

#[test]
fn a_zero_count_delete_out_of_order_or_past_the_end_is_still_refused() {
    assert!(content_of_1_1("d1 2\nd1 0\n").is_err());
    assert!(content_of_1_1("d9 0\n").is_err());
}
