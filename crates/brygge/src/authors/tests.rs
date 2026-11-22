//! Level 1 (RFC 019 D-4 §0): the map parser, matching, and identity collection, over hand-built data —
//! no artifact, no `git`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;

use brygge_ir::{
    Annotation, AtomDraft, EpistemicStatus, Identity, ImportProvenance, IrBuilder, MetadataClaims,
    PathOp, RefKind, RefRecord, SourceIdentity, SourceKind, Text,
};

use super::*;

fn id(name: &str, email: Option<&str>) -> Identity {
    Identity {
        name: Text::utf8(name),
        email: email.map(Text::utf8),
    }
}

// ---- parse_map ----------------------------------------------------------------------------------------

#[test]
fn a_bare_name_source_maps_to_a_name_email_target() {
    let map = parse_map(b"deraadt = Theo de Raadt <deraadt@example.invalid>\n").unwrap();
    assert_eq!(map.mappings.len(), 1);
    assert_eq!(map.mappings[0].source, id("deraadt", None));
    assert_eq!(
        map.mappings[0].target,
        id("Theo de Raadt", Some("deraadt@example.invalid"))
    );
}

#[test]
fn a_name_email_source_maps_to_a_name_email_target() {
    let map = parse_map(b"Old Name <old@x> = New Name <new@x>\n").unwrap();
    assert_eq!(map.mappings[0].source, id("Old Name", Some("old@x")));
    assert_eq!(map.mappings[0].target, id("New Name", Some("new@x")));
}

#[test]
fn comments_and_blank_lines_are_ignored() {
    let map = parse_map(b"# a comment\n\n   \nderaadt = Theo de Raadt <t@x>  # trailing comment\n")
        .unwrap();
    assert_eq!(map.mappings.len(), 1);
    assert_eq!(map.mappings[0].target, id("Theo de Raadt", Some("t@x")));
}

#[test]
fn a_line_with_no_equals_is_refused_with_its_line_number() {
    let err = parse_map(b"deraadt Theo de Raadt <t@x>\n").unwrap_err();
    assert_eq!(err.len(), 1);
    assert!(err[0].contains("line 1"), "{err:?}");
    assert!(err[0].contains('='), "{err:?}");
}

#[test]
fn an_empty_source_side_is_refused() {
    let err = parse_map(b" = New Name <new@x>\n").unwrap_err();
    assert!(err[0].contains("line 1"), "{err:?}");
}

#[test]
fn a_target_with_no_email_is_refused() {
    let err = parse_map(b"deraadt = New Name\n").unwrap_err();
    assert!(err[0].contains("line 1"), "{err:?}");
    assert!(err[0].contains("email"), "{err:?}");
}

#[test]
fn the_same_source_given_twice_is_refused() {
    let err = parse_map(b"deraadt = A <a@x>\nderaadt = B <b@x>\n").unwrap_err();
    assert_eq!(err.len(), 1);
    assert!(err[0].contains("line 2"), "{err:?}");
}

#[test]
fn a_target_containing_a_newline_marker_is_refused() {
    // The parser only ever sees one text line at a time, so a literal LF cannot appear in `s`; this
    // proves the `<`/`>`/NUL guard instead, which a crafted name or email can carry.
    let err = parse_map(b"deraadt = New<Name> <new@x>\n").unwrap_err();
    assert!(err[0].contains("line 1"), "{err:?}");
}

#[test]
fn non_utf8_bytes_are_refused_outright() {
    let err = parse_map(&[0x64, 0x65, 0x72, 0xFF, b'\n']).unwrap_err();
    assert_eq!(err.len(), 1);
    assert!(err[0].contains("UTF-8"), "{err:?}");
}

#[test]
fn every_problem_in_the_file_is_listed_not_just_the_first() {
    let err = parse_map(b"no-equals-here\nderaadt =\n").unwrap_err();
    assert_eq!(err.len(), 2);
    assert!(err[0].contains("line 1"), "{err:?}");
    assert!(err[1].contains("line 2"), "{err:?}");
}

// ---- lookup: no fuzzy matching --------------------------------------------------------------------

#[test]
fn a_bare_name_source_never_matches_an_identity_with_an_email() {
    let map = parse_map(b"deraadt = A <a@x>\n").unwrap();
    assert_eq!(lookup(&map, &id("deraadt", Some("deraadt@real"))), None);
    assert_eq!(lookup(&map, &id("deraadt", None)), Some(0));
}

#[test]
fn a_name_email_source_never_matches_a_bare_name_identity() {
    let map = parse_map(b"Old <old@x> = New <new@x>\n").unwrap();
    assert_eq!(lookup(&map, &id("Old", None)), None);
    assert_eq!(lookup(&map, &id("Old", Some("old@x"))), Some(0));
}

#[test]
fn matching_is_exact_byte_comparison() {
    let map = parse_map(b"Old <old@x> = New <new@x>\n").unwrap();
    assert_eq!(
        lookup(&map, &id("old", Some("old@x"))),
        None,
        "case differs"
    );
    assert_eq!(
        lookup(&map, &id("Old ", Some("old@x"))),
        None,
        "whitespace differs"
    );
}

// ---- collect_identities ---------------------------------------------------------------------------

fn blank_provenance() -> ImportProvenance {
    ImportProvenance {
        source: SourceIdentity {
            kind: SourceKind::Git,
            repo_id: b"r".to_vec(),
            atom_id: b"r".to_vec(),
            signatures: Vec::new(),
            extras: Vec::new(),
        },
        brygge_version: "0.1.0".into(),
        decoder: "brygge-decode-git".into(),
        decoder_version: "0.1.0".into(),
        params: BTreeMap::new(),
    }
}

fn src(atom: &[u8]) -> SourceIdentity {
    SourceIdentity {
        kind: SourceKind::Git,
        repo_id: b"r".to_vec(),
        atom_id: atom.to_vec(),
        signatures: Vec::new(),
        extras: Vec::new(),
    }
}

#[test]
fn every_role_is_counted_once_per_atom_or_ref() {
    let mut b = IrBuilder::new(blank_provenance());
    let blob = b.add_blob(b"x".to_vec()).unwrap();
    let a1 = b
        .add_atom(AtomDraft {
            parents: vec![],
            ops: vec![PathOp::Add {
                path: "a".into(),
                blob,
                mode: 0o100_644,
                status: EpistemicStatus::Stated,
            }],
            copies: vec![],
            metadata: MetadataClaims {
                author: Some(id("Alice", Some("alice@x"))),
                committer: Some(id("Alice", Some("alice@x"))),
                ..Default::default()
            },
            source: src(b"c1"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    let a2 = b
        .add_atom(AtomDraft {
            parents: vec![a1],
            ops: vec![],
            copies: vec![],
            metadata: MetadataClaims {
                author: Some(id("deraadt", None)),
                ..Default::default()
            },
            source: src(b"c2"),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    b.add_ref(RefRecord {
        name: "v1".into(),
        kind: RefKind::Tag,
        target: a2,
        status: EpistemicStatus::Stated,
        source: None,
        annotation: Some(Annotation {
            tagger: Some(id("Alice", Some("alice@x"))),
            time: None,
            message: None,
        }),
    })
    .unwrap();
    let ir = b.finish().unwrap();

    let identities = collect_identities(&ir);
    assert_eq!(identities.len(), 2);
    // Alice: 1 author, 1 committer, 1 tagger = 3 total — sorts first.
    assert_eq!(identities[0].identity, id("Alice", Some("alice@x")));
    assert_eq!(identities[0].author, 1);
    assert_eq!(identities[0].committer, 1);
    assert_eq!(identities[0].tagger, 1);
    assert_eq!(identities[1].identity, id("deraadt", None));
    assert_eq!(identities[1].author, 1);
    assert_eq!(identities[1].committer, 0);
    assert_eq!(identities[1].tagger, 0);
}

#[test]
fn a_bare_name_and_a_name_with_an_email_are_distinct_identities() {
    let mut b = IrBuilder::new(blank_provenance());
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
        metadata: MetadataClaims {
            author: Some(id("deraadt", None)),
            ..Default::default()
        },
        source: src(b"c1"),
        status: EpistemicStatus::Stated,
    })
    .unwrap();
    let ir = b.finish().unwrap();
    let identities = collect_identities(&ir);
    assert_eq!(identities.len(), 1);
    assert_eq!(
        lookup(
            &parse_map(b"deraadt <deraadt@x> = A <a@x>\n").unwrap(),
            &identities[0].identity
        ),
        None
    );
}

#[test]
fn identity_as_map_source_round_trips_through_parse_source_side() {
    let a = id("Alice Example", Some("alice@example.org"));
    let rendered = identity_as_map_source(&a);
    assert_eq!(rendered, "Alice Example <alice@example.org>");
    let bare = id("deraadt", None);
    assert_eq!(identity_as_map_source(&bare), "deraadt");
}
