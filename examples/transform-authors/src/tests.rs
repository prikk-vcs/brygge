#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;

use brygge_ir::model::{ChangeAtom, MetadataClaims, SourceIdentity, SourceKind};
use brygge_ir::status::EpistemicStatus;
use brygge_ir::{Identity, Text};

use super::*;

fn identity(name: &str, email: &str) -> Identity {
    Identity {
        name: Text::utf8(name),
        email: Some(Text::utf8(email)),
    }
}

fn source(n: u8) -> SourceIdentity {
    SourceIdentity {
        kind: SourceKind::Git,
        repo_id: vec![0],
        atom_id: vec![n],
        signatures: Vec::new(),
        extras: Vec::new(),
    }
}

fn one_atom_ir(author: Identity, committer: Identity) -> brygge_ir::Ir {
    let mut builder = brygge_ir::IrBuilder::new(brygge_ir::ImportProvenance {
        decoder: "brygge-decode-git".to_string(),
        decoder_version: "0".to_string(),
        brygge_version: "0".to_string(),
        source: source(1),
        params: BTreeMap::new(),
    });
    builder
        .add_atom(brygge_ir::AtomDraft {
            parents: Vec::new(),
            ops: Vec::new(),
            copies: Vec::new(),
            metadata: MetadataClaims {
                author: Some(author),
                author_time: None,
                committer: Some(committer),
                commit_time: None,
                message: None,
            },
            source: source(1),
            status: EpistemicStatus::Stated,
        })
        .unwrap();
    builder.finish().unwrap()
}

fn decoded_of(ir: brygge_ir::Ir) -> brygge_ir::Decoded {
    let bytes = brygge_ir::to_bytes(&ir);
    brygge_ir::from_bytes(&bytes).unwrap()
}

#[test]
fn parse_map_reads_a_mapping_line() {
    let map = parse_map(b"Old Name <old@example.com> = New Name <new@example.com>\n").unwrap();
    assert_eq!(map.len(), 1);
    assert_eq!(map[0].old, identity("Old Name", "old@example.com"));
    assert_eq!(map[0].new, identity("New Name", "new@example.com"));
}

#[test]
fn parse_map_skips_blank_and_comment_lines() {
    let map = parse_map(b"\n# a comment\n   \nOld <a@x> = New <b@x>\n").unwrap();
    assert_eq!(map.len(), 1);
}

#[test]
fn parse_map_rejects_a_line_with_no_equals() {
    assert!(parse_map(b"Old <a@x>\n").is_err());
}

#[test]
fn parse_map_rejects_a_side_with_no_angle_brackets() {
    assert!(parse_map(b"Old = New <b@x>\n").is_err());
    assert!(parse_map(b"Old <a@x> = New\n").is_err());
}

#[test]
fn parse_map_rejects_an_empty_name_or_email() {
    assert!(parse_map(b" <a@x> = New <b@x>\n").is_err());
    assert!(parse_map(b"Old <> = New <b@x>\n").is_err());
}

#[test]
fn transform_remaps_a_matching_author_and_committer() {
    let ir = one_atom_ir(identity("Old", "old@x"), identity("Old", "old@x"));
    let input = decoded_of(ir);
    let map = vec![Mapping {
        old: identity("Old", "old@x"),
        new: identity("New", "new@x"),
    }];
    let output = transform(&input, &map, b"the map bytes").unwrap();
    let atom: &ChangeAtom = &output.atoms[0];
    assert_eq!(atom.metadata.author, Some(identity("New", "new@x")));
    assert_eq!(atom.metadata.committer, Some(identity("New", "new@x")));
    assert_eq!(output.transforms.len(), 1);
    assert_eq!(output.transforms[0].name, "authors");
    assert_eq!(
        output.transforms[0]
            .params
            .get("map_entries")
            .map(String::as_str),
        Some("1")
    );
    assert!(output.transforms[0].params.contains_key("map_sha256"));
    assert!(
        !output.transforms[0]
            .params
            .values()
            .any(|v| v.contains("the map bytes")),
        "the map's own path/content must never appear in params, only its hash and count"
    );
}

#[test]
fn transform_leaves_a_non_matching_atom_untouched() {
    let ir = one_atom_ir(identity("Other", "other@x"), identity("Other", "other@x"));
    let input = decoded_of(ir);
    let map = vec![Mapping {
        old: identity("Old", "old@x"),
        new: identity("New", "new@x"),
    }];
    let output = transform(&input, &map, b"map").unwrap();
    // `Edit::finish` always records the step it was asked to make (RFC 018 D-6's own "accept" lean on
    // an empty step), but the atom itself carries no mark: nothing about it actually changed.
    assert_eq!(output.transforms.len(), 1);
    let atom: &ChangeAtom = &output.atoms[0];
    assert_eq!(atom.status, EpistemicStatus::Stated);
    assert_eq!(
        atom.metadata.author,
        Some(identity("Other", "other@x")),
        "an atom the map does not match must pass through byte-identical"
    );
}

#[test]
fn transform_output_passes_check() {
    let ir = one_atom_ir(identity("Old", "old@x"), identity("Keep", "keep@x"));
    let input = decoded_of(ir);
    let map = vec![Mapping {
        old: identity("Old", "old@x"),
        new: identity("New", "new@x"),
    }];
    let output_ir = transform(&input, &map, b"map").unwrap();
    let output_bytes = brygge_ir::to_bytes(&output_ir);
    let output = brygge_ir::from_bytes(&output_bytes).unwrap();
    brygge_ir::transform::check(&input, &output, "authors").unwrap();
}
