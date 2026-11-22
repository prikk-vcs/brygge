//! Level 1: unit tests for the pure trailer/identity helpers (no process, no artifact).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use brygge_ir::Text;

#[test]
fn a_present_identity_needs_nothing_supplied_or_altered() {
    let identity = Identity {
        name: Text::utf8("A U Thor"),
        email: Some(Text::utf8("a@example.com")),
    };
    let mut supplied = Supplied::default();
    let mut altered = Altered::default();
    let line = identity_line(Some(&identity), &mut supplied, &mut altered, "author");
    assert_eq!(line, "A U Thor <a@example.com>");
    assert!(supplied.trailer().is_none());
    assert!(altered.trailer().is_none());
}

#[test]
fn a_missing_email_is_supplied_as_empty_and_marked() {
    // Review 136 F-1: only the email is missing here, so the mark must name `author-email`, not the
    // bare `author` — that would claim brygge invented the whole identity, which it did not (the name
    // is the IR's own).
    let identity = Identity {
        name: Text::utf8("A U Thor"),
        email: None,
    };
    let mut supplied = Supplied::default();
    let mut altered = Altered::default();
    let line = identity_line(Some(&identity), &mut supplied, &mut altered, "author");
    assert_eq!(line, "A U Thor <>");
    assert_eq!(supplied.trailer().unwrap(), "Brygge-Supplied: author-email");
    assert!(altered.trailer().is_none());
}

#[test]
fn a_missing_identity_is_supplied_under_the_bare_field_not_email() {
    // The other half of F-1: the whole identity absent (distinct from only its email) is marked under
    // the bare field name.
    let mut supplied = Supplied::default();
    let mut altered = Altered::default();
    let line = identity_line(None, &mut supplied, &mut altered, "tagger");
    assert_eq!(line, "unknown <>");
    assert_eq!(supplied.trailer().unwrap(), "Brygge-Supplied: tagger");
    assert!(altered.trailer().is_none());
}

#[test]
fn a_missing_identity_entirely_is_supplied_and_marked() {
    let mut supplied = Supplied::default();
    let mut altered = Altered::default();
    let line = identity_line(None, &mut supplied, &mut altered, "committer");
    assert_eq!(line, "unknown <>");
    assert_eq!(supplied.trailer().unwrap(), "Brygge-Supplied: committer");
    assert!(altered.trailer().is_none());
}

#[test]
fn a_newline_in_a_name_is_folded_to_a_space_and_marked_altered() {
    let identity = Identity {
        name: Text::utf8("A U\nThor"),
        email: Some(Text::utf8("a@example.com")),
    };
    let mut supplied = Supplied::default();
    let mut altered = Altered::default();
    let line = identity_line(Some(&identity), &mut supplied, &mut altered, "author");
    assert_eq!(line, "A U Thor <a@example.com>");
    assert!(supplied.trailer().is_none());
    assert_eq!(
        altered.trailer().unwrap(),
        "Brygge-Altered: author-name (newline folded)"
    );
}

#[test]
fn a_non_utf8_name_is_replaced_with_unknown_and_marked_altered() {
    let identity = Identity {
        name: Text {
            bytes: vec![0xff, 0xfe],
            encoding: None,
        },
        email: Some(Text::utf8("a@example.com")),
    };
    let mut supplied = Supplied::default();
    let mut altered = Altered::default();
    let line = identity_line(Some(&identity), &mut supplied, &mut altered, "author");
    assert_eq!(line, "unknown <a@example.com>");
    assert!(supplied.trailer().is_none());
    assert_eq!(
        altered.trailer().unwrap(),
        "Brygge-Altered: author-name (not UTF-8, replaced)"
    );
}

#[test]
fn a_non_utf8_email_is_replaced_with_empty_and_marked_altered() {
    let identity = Identity {
        name: Text::utf8("A U Thor"),
        email: Some(Text {
            bytes: vec![0xff, 0xfe],
            encoding: None,
        }),
    };
    let mut supplied = Supplied::default();
    let mut altered = Altered::default();
    let line = identity_line(Some(&identity), &mut supplied, &mut altered, "committer");
    assert_eq!(line, "A U Thor <>");
    assert!(supplied.trailer().is_none());
    assert_eq!(
        altered.trailer().unwrap(),
        "Brygge-Altered: committer-email (not UTF-8, replaced)"
    );
}

#[test]
fn angle_brackets_in_an_email_are_removed_and_marked_altered() {
    let identity = Identity {
        name: Text::utf8("A U Thor"),
        email: Some(Text::utf8("a<b>@example.com")),
    };
    let mut supplied = Supplied::default();
    let mut altered = Altered::default();
    let line = identity_line(Some(&identity), &mut supplied, &mut altered, "author");
    assert_eq!(line, "A U Thor <ab@example.com>");
    assert_eq!(
        altered.trailer().unwrap(),
        "Brygge-Altered: author-email (angle bracket removed)"
    );
}

#[test]
fn a_nul_in_a_name_is_removed_and_marked_altered() {
    let identity = Identity {
        name: Text::utf8("A U\0Thor"),
        email: Some(Text::utf8("a@example.com")),
    };
    let mut supplied = Supplied::default();
    let mut altered = Altered::default();
    let line = identity_line(Some(&identity), &mut supplied, &mut altered, "author");
    assert_eq!(line, "A UThor <a@example.com>");
    assert_eq!(
        altered.trailer().unwrap(),
        "Brygge-Altered: author-name (NUL removed)"
    );
}

#[test]
fn multiple_alterations_on_one_field_are_combined_in_one_reason() {
    let identity = Identity {
        name: Text::utf8("A\nU <Thor>"),
        email: Some(Text::utf8("a@example.com")),
    };
    let mut supplied = Supplied::default();
    let mut altered = Altered::default();
    let _ = identity_line(Some(&identity), &mut supplied, &mut altered, "author");
    assert_eq!(
        altered.trailer().unwrap(),
        "Brygge-Altered: author-name (newline folded, angle bracket removed)"
    );
}

#[test]
fn a_present_time_with_a_negative_offset_formats_correctly() {
    let time = Time {
        seconds: 1_700_000_000,
        offset_minutes: Some(-330),
    };
    let mut supplied = Supplied::default();
    let w = when(Some(time), &mut supplied, "author-time", "author-time-zone");
    assert_eq!(w, "1700000000 -0530");
    assert!(supplied.trailer().is_none());
}

#[test]
fn a_missing_time_zone_is_supplied_as_plus_0000_and_marked() {
    let time = Time {
        seconds: 1_700_000_000,
        offset_minutes: None,
    };
    let mut supplied = Supplied::default();
    let w = when(Some(time), &mut supplied, "author-time", "author-time-zone");
    assert_eq!(w, "1700000000 +0000");
    assert_eq!(
        supplied.trailer().unwrap(),
        "Brygge-Supplied: author-time-zone"
    );
}

#[test]
fn a_missing_time_entirely_is_supplied_as_epoch_zero_and_both_marked() {
    let mut supplied = Supplied::default();
    let w = when(None, &mut supplied, "author-time", "author-time-zone");
    assert_eq!(w, "0 +0000");
    assert_eq!(
        supplied.trailer().unwrap(),
        "Brygge-Supplied: author-time, author-time-zone"
    );
}

#[test]
fn an_ordinary_path_is_unquoted() {
    assert_eq!(path_token("src/main.rs"), "src/main.rs");
    assert_eq!(
        path_token("a path with spaces.txt"),
        "a path with spaces.txt"
    );
}

#[test]
fn a_path_starting_with_a_quote_is_quoted_and_escaped() {
    assert_eq!(path_token("\"quoted\".txt"), "\"\\\"quoted\\\".txt\"");
}

#[test]
fn a_path_with_a_newline_is_quoted_and_escaped() {
    assert_eq!(path_token("a\nb"), "\"a\\nb\"");
}

#[test]
fn a_stated_status_has_no_derived_trailer() {
    assert!(derived_trailer("atom", &EpistemicStatus::Stated).is_none());
}

#[test]
fn a_derived_status_renders_its_kind_and_params() {
    use brygge_ir::{Derivation, DerivationKind};
    use std::collections::BTreeMap;
    let mut params = BTreeMap::new();
    params.insert("window_secs".to_string(), "180".to_string());
    let status = EpistemicStatus::Derived(Derivation {
        kind: DerivationKind::ReconstructedChangeset,
        by: "brygge-decode-cvs".to_string(),
        decoder_version: "0.5.0".to_string(),
        params,
        confidence: None,
    });
    let line = derived_trailer("atom", &status).unwrap();
    assert_eq!(
        line,
        "Brygge-Derived: atom reconstructed-changeset window_secs=180"
    );
}

// ---- review 137 F-7: one readable source-id form per source kind, with a hex: fallback -------------

#[test]
fn a_git_commit_id_is_forty_lowercase_hex_no_prefix() {
    let id = [0x01u8; 20];
    assert_eq!(
        source_id_text(&SourceKind::Git, &id),
        "0101010101010101010101010101010101010101"
    );
}

#[test]
fn an_hg_node_is_forty_lowercase_hex_no_prefix() {
    let id = [0xabu8; 20];
    assert_eq!(
        source_id_text(&SourceKind::Hg, &id),
        "abababababababababababababababababababab"
    );
}

#[test]
fn a_git_id_of_the_wrong_length_falls_back_to_hex_prefixed() {
    assert_eq!(source_id_text(&SourceKind::Git, &[0x01, 0x02]), "hex:0102");
}

#[test]
fn an_svn_revision_is_r_decimal() {
    let id = 1234u64.to_be_bytes();
    assert_eq!(source_id_text(&SourceKind::Svn, &id), "r1234");
}

#[test]
fn an_svn_id_of_the_wrong_length_falls_back_to_hex_prefixed() {
    assert_eq!(source_id_text(&SourceKind::Svn, &[0x00, 0x01]), "hex:0001");
}

/// Review 147 F-4: `--branch-lines`' extended SVN form, `r<N> <root>` — round trips through
/// `parse_source_id`, the exact inverse `source_id_text` is specified against.
#[test]
fn an_svn_branch_lines_id_round_trips_with_its_root() {
    let mut id = 1234u64.to_be_bytes().to_vec();
    id.extend_from_slice(b"branches/1.5");
    let text = source_id_text(&SourceKind::Svn, &id);
    assert_eq!(text, "r1234 branches/1.5");
    assert_eq!(parse_source_id(&SourceKind::Svn, &text), Some(id));
}

/// The one fallback the handoff names explicitly: a root containing a space (which the `r<N> <root>`
/// form's own separator could not be told apart from) falls back to `hex:`, and still round trips.
#[test]
fn an_svn_branch_lines_root_with_a_space_falls_back_to_hex() {
    let mut id = 5u64.to_be_bytes().to_vec();
    id.extend_from_slice(b"branches/with space");
    let text = source_id_text(&SourceKind::Svn, &id);
    assert!(text.starts_with("hex:"), "{text}");
    assert_eq!(parse_source_id(&SourceKind::Svn, &text), Some(id));
}

#[test]
fn a_cvs_branch_point_id_is_written_as_is() {
    let id = b"branch-point:OPENBSD_2_0";
    assert_eq!(
        source_id_text(&SourceKind::Cvs, id),
        "branch-point:OPENBSD_2_0"
    );
}

#[test]
fn a_cvs_changeset_id_is_one_pair_per_line_as_a_continuation() {
    let id = b"a.c@1.1\nb.c@1.2";
    assert_eq!(source_id_text(&SourceKind::Cvs, id), "a.c@1.1\n b.c@1.2");
}

#[test]
fn a_cvs_changeset_id_with_many_pairs_continues_each_on_its_own_line() {
    let id = b"a.c@1.1\nb.c@1.2\nc.c@1.3";
    assert_eq!(
        source_id_text(&SourceKind::Cvs, id),
        "a.c@1.1\n b.c@1.2\n c.c@1.3"
    );
}

#[test]
fn a_non_utf8_cvs_id_falls_back_to_hex() {
    let id = [0xffu8, 0xfeu8];
    assert_eq!(source_id_text(&SourceKind::Cvs, &id), "hex:fffe");
}

#[test]
fn a_cvs_pair_missing_an_at_sign_falls_back_to_hex() {
    let id = b"not-a-pair";
    let expected_hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(
        source_id_text(&SourceKind::Cvs, id),
        format!("hex:{expected_hex}")
    );
}

#[test]
fn a_cvs_pair_with_a_leading_space_falls_back_to_hex() {
    let id = b"a.c@1.1\n b.c@1.2"; // the second "pair" already looks like a continuation line
    let expected_hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(
        source_id_text(&SourceKind::Cvs, id),
        format!("hex:{expected_hex}")
    );
}

#[test]
fn an_empty_cvs_pair_falls_back_to_hex() {
    let id = b"a.c@1.1\n\nb.c@1.2"; // two adjacent LFs: an empty "pair"
    let expected_hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(
        source_id_text(&SourceKind::Cvs, id),
        format!("hex:{expected_hex}")
    );
}

#[test]
fn an_other_source_kind_uses_the_printable_ascii_or_hex_fallback() {
    assert_eq!(
        source_id_text(&SourceKind::Other("bzr".to_string()), b"abcDEF-0123"),
        "abcDEF-0123"
    );
    let id = [0x01u8, 0xffu8, 0x00u8, 0x41u8];
    assert_eq!(
        source_id_text(&SourceKind::Other("bzr".to_string()), &id),
        "hex:01ff0041"
    );
}

#[test]
fn an_empty_id_falls_back_to_hex_not_an_empty_string() {
    // An empty value is not "every byte printable ASCII" (vacuous truth) in a way that should ever print
    // as an empty, invisible trailer value — write it as hex (the empty string) explicitly instead.
    assert_eq!(
        source_id_text(&SourceKind::Other("bzr".to_string()), &[]),
        "hex:"
    );
}

#[test]
fn truncate_for_refusal_leaves_a_short_id_unchanged() {
    assert_eq!(truncate_for_refusal("short"), "short");
}

#[test]
fn truncate_for_refusal_cuts_a_long_cvs_id_to_eighty_chars_plus_ellipsis() {
    let long_id: String = (0..200).map(|i| format!("f{i}.c@1.1\n")).collect();
    let truncated = truncate_for_refusal(&long_id);
    assert_eq!(truncated.chars().count(), 81);
    assert!(truncated.ends_with('…'));
    assert!(long_id.starts_with(&truncated[..truncated.len() - '…'.len_utf8()]));
}

#[test]
fn source_kind_labels_match_the_trailer_vocabulary() {
    assert_eq!(source_kind_label(&SourceKind::Git), "git");
    assert_eq!(source_kind_label(&SourceKind::Hg), "hg");
    assert_eq!(source_kind_label(&SourceKind::Svn), "svn");
    assert_eq!(source_kind_label(&SourceKind::Cvs), "cvs");
}

#[test]
fn supplied_and_altered_fold_into_per_field_totals() {
    let mut supplied = Supplied::default();
    supplied.mark("author");
    supplied.mark("author");
    supplied.mark("committer");
    let mut totals: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    supplied.fold_into(&mut totals);
    assert_eq!(totals.get("author"), Some(&2));
    assert_eq!(totals.get("committer"), Some(&1));

    let mut altered = Altered::default();
    altered.mark("author-name", "not UTF-8, replaced");
    let mut altered_totals: std::collections::BTreeMap<String, u64> =
        std::collections::BTreeMap::new();
    altered.fold_into(&mut altered_totals);
    assert_eq!(altered_totals.get("author-name"), Some(&1));
}
