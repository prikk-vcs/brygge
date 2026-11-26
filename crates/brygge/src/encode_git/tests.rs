//! Level 1: unit tests for the ref mapping, default-branch choice, pre-flight refusals, and the stream
//! writer itself, all over hand-built `Ir`s (no process, no real repository — `tests/encode_git_fast_import.rs`
//! is the level-2 proof against a real `git fast-import`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use brygge_ir::{
    Annotation, AtomDraft, ChangedPart, Content, Derivation, DerivationKind, Identity,
    ImportProvenance, IrBuilder, MetadataClaims, SourceIdentity, SourceKind, Text, Time,
};

fn source(kind: SourceKind, atom_id: &[u8]) -> SourceIdentity {
    SourceIdentity {
        kind,
        repo_id: Vec::new(),
        atom_id: atom_id.to_vec(),
        signatures: Vec::new(),
        extras: Vec::new(),
    }
}

fn provenance(kind: SourceKind) -> ImportProvenance {
    ImportProvenance {
        source: source(kind, b"repo"),
        brygge_version: "0.0.0".to_string(),
        decoder: "test".to_string(),
        decoder_version: "0.0.0".to_string(),
        params: BTreeMap::new(),
    }
}

// ---- git_ref_path (D-3) -------------------------------------------------------------------------------

#[test]
fn every_ref_kind_maps_to_its_own_namespace() {
    let mk = |kind: RefKind| RefRecord {
        name: "x".to_string(),
        kind,
        target: brygge_ir::AtomId([0u8; 32]),
        status: brygge_ir::EpistemicStatus::Stated,
        source: None,
        annotation: None,
    };
    assert_eq!(git_ref_path(&mk(RefKind::Branch)), "refs/heads/x");
    assert_eq!(git_ref_path(&mk(RefKind::Tag)), "refs/tags/x");
    assert_eq!(git_ref_path(&mk(RefKind::Bookmark)), "refs/heads/x");
    assert_eq!(git_ref_path(&mk(RefKind::NamedBranch)), "refs/heads/x");
    assert_eq!(
        git_ref_path(&mk(RefKind::Other("stash".to_string()))),
        "refs/brygge/other/x"
    );
}

// ---- ref_collisions / bad_modes / non_utf8_tag_messages (D-7) ----------------------------------------

#[test]
fn no_collision_among_distinct_refs() {
    let refs = vec![
        RefRecord {
            name: "main".to_string(),
            kind: RefKind::Branch,
            target: brygge_ir::AtomId([0u8; 32]),
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        },
        RefRecord {
            name: "main".to_string(),
            kind: RefKind::Tag,
            target: brygge_ir::AtomId([0u8; 32]),
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        },
    ];
    assert!(ref_collisions(&refs).is_empty());
}

#[test]
fn a_bookmark_and_a_named_branch_of_one_name_collide() {
    let refs = vec![
        RefRecord {
            name: "stable".to_string(),
            kind: RefKind::NamedBranch,
            target: brygge_ir::AtomId([0u8; 32]),
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        },
        RefRecord {
            name: "stable".to_string(),
            kind: RefKind::Bookmark,
            target: brygge_ir::AtomId([0u8; 32]),
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        },
    ];
    let collisions = ref_collisions(&refs);
    assert_eq!(collisions.len(), 1);
    let msg = collisions[0].to_string();
    assert!(msg.contains("refs/heads/stable"), "{msg}");
    assert!(msg.contains("NamedBranch"), "{msg}");
    assert!(msg.contains("Bookmark"), "{msg}");
}

fn minimal_ir_with_mode(mode: u32) -> brygge_ir::Ir {
    let mut builder = IrBuilder::new(provenance(SourceKind::Git));
    let blob = builder.add_blob(b"hello\n".to_vec()).unwrap();
    let atom_id = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![brygge_ir::PathOp::Add {
                path: "f".to_string(),
                content: Content::blob(blob, mode),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims {
                author: Some(Identity {
                    name: Text::utf8("A"),
                    email: Some(Text::utf8("a@example.com")),
                }),
                author_time: Some(Time {
                    seconds: 1,
                    offset_minutes: Some(0),
                }),
                committer: None,
                commit_time: None,
                message: Some(Text::utf8("m")),
            },
            source: source(SourceKind::Git, b"r1"),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    builder
        .add_ref(RefRecord {
            name: "main".to_string(),
            kind: RefKind::Branch,
            target: atom_id,
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    builder.finish().unwrap()
}

#[test]
fn regular_executable_and_symlink_modes_pass_preflight() {
    for mode in [0o100_644, 0o100_755, 0o120_000] {
        let ir = minimal_ir_with_mode(mode);
        assert!(preflight(&ir).is_empty(), "mode {mode:#o}");
    }
}

#[test]
fn a_gitlink_mode_is_refused_by_name() {
    let ir = minimal_ir_with_mode(0o160_000);
    let refusals = preflight(&ir);
    assert_eq!(refusals.len(), 1);
    let msg = refusals[0].to_string();
    assert!(msg.contains("160000"), "{msg}");
}

#[test]
fn three_refusal_kinds_at_once_are_all_collected() {
    let mut builder = IrBuilder::new(provenance(SourceKind::Git));
    let blob = builder.add_blob(b"x\n".to_vec()).unwrap();
    let atom_id = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![brygge_ir::PathOp::Add {
                path: "f".to_string(),
                content: Content::blob(blob, 0o160_000), // bad mode
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims::default(),
            source: source(SourceKind::Git, b"r1"),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    builder
        .add_ref(RefRecord {
            name: "stable".to_string(),
            kind: RefKind::Branch,
            target: atom_id,
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    builder
        .add_ref(RefRecord {
            name: "stable".to_string(),
            kind: RefKind::NamedBranch,
            target: atom_id,
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    builder
        .add_ref(RefRecord {
            name: "v1".to_string(),
            kind: RefKind::Tag,
            target: atom_id,
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: Some(Annotation {
                tagger: None,
                time: None,
                message: Some(Text {
                    bytes: vec![0xff, 0xfe],
                    encoding: None,
                }),
            }),
        })
        .unwrap();
    let ir = builder.finish().unwrap();
    let refusals = preflight(&ir);
    assert_eq!(refusals.len(), 3, "{refusals:#?}");
    assert!(
        refusals
            .iter()
            .any(|r| matches!(r, Refusal::BadMode { .. }))
    );
    assert!(
        refusals
            .iter()
            .any(|r| matches!(r, Refusal::RefCollision { .. }))
    );
    assert!(
        refusals
            .iter()
            .any(|r| matches!(r, Refusal::NonUtf8TagMessage { .. }))
    );
}

// ---- write_stream's gitlink form (RFC 021 D-5, batch G) ----------------------------------------------

#[test]
fn a_submodule_passes_preflight_and_is_never_refused() {
    let mut builder = IrBuilder::new(provenance(SourceKind::Git));
    let atom_id = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![brygge_ir::PathOp::Add {
                path: "vendor/lib".to_string(),
                content: Content::submodule(vec![0xab; 20]),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims::default(),
            source: source(SourceKind::Git, b"r1"),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    builder
        .add_ref(RefRecord {
            name: "main".to_string(),
            kind: RefKind::Branch,
            target: atom_id,
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    let ir = builder.finish().unwrap();
    assert!(
        preflight(&ir).is_empty(),
        "a submodule is written, not refused, since batch G"
    );
}

#[test]
fn write_stream_writes_a_submodule_as_a_gitlink_m_line() {
    let mut builder = IrBuilder::new(provenance(SourceKind::Git));
    let atom_id = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![brygge_ir::PathOp::Add {
                path: "vendor/lib".to_string(),
                content: Content::submodule(vec![0xab; 20]),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims::default(),
            source: source(SourceKind::Git, b"r1"),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    builder
        .add_ref(RefRecord {
            name: "main".to_string(),
            kind: RefKind::Branch,
            target: atom_id,
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    let ir = builder.finish().unwrap();

    let mut out = Vec::new();
    write_stream(&ir, &[0u8; 32], &mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    // The gitlink form: an object id written directly, never a `:mark` (this commit was never itself
    // written by this stream — it is a pinned reference to one that may not even exist here).
    assert!(
        text.contains(&format!("M 160000 {} vendor/lib", "ab".repeat(20))),
        "{text}"
    );
}

// ---- choose_default_branch (D-3) ---------------------------------------------------------------------

fn ir_with_branch_names(names: &[&str]) -> brygge_ir::Ir {
    let mut builder = IrBuilder::new(provenance(SourceKind::Git));
    let blob = builder.add_blob(b"x\n".to_vec()).unwrap();
    let atom_id = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![brygge_ir::PathOp::Add {
                path: "f".to_string(),
                content: Content::blob(blob, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims::default(),
            source: source(SourceKind::Git, b"r1"),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    for name in names {
        builder
            .add_ref(RefRecord {
                name: name.to_string(),
                kind: RefKind::Branch,
                target: atom_id,
                status: brygge_ir::EpistemicStatus::Stated,
                source: None,
                annotation: None,
            })
            .unwrap();
    }
    builder.finish().unwrap()
}

#[test]
fn trunk_wins_over_main_and_master() {
    let ir = ir_with_branch_names(&["feature", "master", "main", "trunk"]);
    let chosen = choose_default_branch(&ir);
    assert_eq!(chosen.name, "trunk");
    assert!(chosen.existed);
}

#[test]
fn main_wins_when_trunk_is_absent() {
    let ir = ir_with_branch_names(&["feature", "master", "main"]);
    assert_eq!(choose_default_branch(&ir).name, "main");
}

#[test]
fn the_first_branch_in_canonical_order_is_used_when_none_of_the_four_names_exist() {
    // "Canonical order" sorts refs by name, so "develop" (d < f) is first here regardless of insertion
    // order — confirmed against the real `IrBuilder`, not assumed.
    let ir = ir_with_branch_names(&["feature", "develop"]);
    let chosen = choose_default_branch(&ir);
    assert_eq!(chosen.name, "develop");
    assert!(chosen.existed);
}

#[test]
fn tags_only_gives_main_that_does_not_exist() {
    let mut builder = IrBuilder::new(provenance(SourceKind::Git));
    let blob = builder.add_blob(b"x\n".to_vec()).unwrap();
    let atom_id = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![brygge_ir::PathOp::Add {
                path: "f".to_string(),
                content: Content::blob(blob, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims::default(),
            source: source(SourceKind::Git, b"r1"),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    builder
        .add_ref(RefRecord {
            name: "v1".to_string(),
            kind: RefKind::Tag,
            target: atom_id,
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    let ir = builder.finish().unwrap();
    let chosen = choose_default_branch(&ir);
    assert_eq!(chosen.name, "main");
    assert!(!chosen.existed);
}

// ---- write_stream (D-1/D-2/D-3/D-5) ------------------------------------------------------------------

#[test]
fn a_commit_carries_encoded_source_id_and_artifact_trailers() {
    // `Brygge-Encoded`'s source label comes from `ir.provenance.source.kind` (one IR, one import), so
    // the provenance's own kind must match the atom's — Svn here, consistent with the atom below.
    let mut builder = IrBuilder::new(provenance(SourceKind::Svn));
    let blob = builder.add_blob(b"hello\n".to_vec()).unwrap();
    let atom_id = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![brygge_ir::PathOp::Add {
                path: "f.txt".to_string(),
                content: Content::blob(blob, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims {
                author: Some(Identity {
                    name: Text::utf8("A U Thor"),
                    email: Some(Text::utf8("a@example.com")),
                }),
                author_time: Some(Time {
                    seconds: 1_700_000_000,
                    offset_minutes: Some(0),
                }),
                committer: None,
                commit_time: None,
                message: Some(Text::utf8("hello")),
            },
            // The real convention (machine-output.md): an SVN revision number as an 8-byte big-endian
            // integer, which review 137 F-7 renders as the readable "r42", not the literal text "42"
            // (which would be a different, wrong 8-byte value) and not a hex dump either.
            source: source(SourceKind::Svn, &42u64.to_be_bytes()),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    builder
        .add_ref(RefRecord {
            name: "trunk".to_string(),
            kind: RefKind::Branch,
            target: atom_id,
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    let ir = builder.finish().unwrap();
    assert!(preflight(&ir).is_empty());

    let mut out = Vec::new();
    let summary = write_stream(&ir, &[0x11; 32], &mut out).unwrap();
    assert_eq!(summary.commits, 1);
    assert_eq!(summary.refs, 1);
    assert_eq!(summary.tags, 0);

    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("Brygge-Encoded: brygge "), "{text}");
    assert!(text.contains("from svn"), "{text}");
    // Review 137 F-7: an SVN revision (42, as an 8-byte big-endian integer per machine-output.md) is
    // written as the readable `r<decimal>` a Subversion user would recognize, not a hex dump.
    assert!(text.contains("Brygge-Source-Id: r42"), "{text}");
    assert!(
        text.contains("Brygge-Artifact: sha256:1111111111111111111111111111111111111111111111111111111111111111"),
        "{text}"
    );
    assert!(text.contains("reset refs/heads/trunk"), "{text}");
    assert!(text.contains(&format!("reset {SCRATCH_REF}")), "{text}");
}

#[test]
fn an_other_ref_is_written_outside_the_branch_and_tag_namespaces() {
    let mut builder = IrBuilder::new(provenance(SourceKind::Hg));
    let blob = builder.add_blob(b"x\n".to_vec()).unwrap();
    let atom_id = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![brygge_ir::PathOp::Add {
                path: "f".to_string(),
                content: Content::blob(blob, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims::default(),
            source: source(SourceKind::Hg, b"r1"),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    builder
        .add_ref(RefRecord {
            name: "closed-heads".to_string(),
            kind: RefKind::Other("evolution".to_string()),
            target: atom_id,
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    let ir = builder.finish().unwrap();
    let mut out = Vec::new();
    write_stream(&ir, &[0u8; 32], &mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(
        text.contains("reset refs/brygge/other/closed-heads"),
        "{text}"
    );
}

#[test]
fn an_annotated_tag_carries_encoded_and_artifact_but_not_source_id() {
    let mut builder = IrBuilder::new(provenance(SourceKind::Git));
    let blob = builder.add_blob(b"x\n".to_vec()).unwrap();
    let atom_id = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![brygge_ir::PathOp::Add {
                path: "f".to_string(),
                content: Content::blob(blob, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims::default(),
            source: source(SourceKind::Git, b"r1"),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    builder
        .add_ref(RefRecord {
            name: "main".to_string(),
            kind: RefKind::Branch,
            target: atom_id,
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    builder
        .add_ref(RefRecord {
            name: "v1".to_string(),
            kind: RefKind::Tag,
            target: atom_id,
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: Some(Annotation {
                tagger: Some(Identity {
                    name: Text::utf8("Tag Ger"),
                    email: Some(Text::utf8("t@example.com")),
                }),
                time: Some(Time {
                    seconds: 2,
                    offset_minutes: Some(0),
                }),
                message: Some(Text::utf8("release")),
            }),
        })
        .unwrap();
    let ir = builder.finish().unwrap();
    let mut out = Vec::new();
    let summary = write_stream(&ir, &[0u8; 32], &mut out).unwrap();
    assert_eq!(summary.tags, 1);
    let text = String::from_utf8(out).unwrap();
    let tag_block = text.split("tag v1\n").nth(1).unwrap();
    assert!(tag_block.contains("Brygge-Encoded:"), "{tag_block}");
    assert!(tag_block.contains("Brygge-Artifact:"), "{tag_block}");
    assert!(!tag_block.contains("Brygge-Source-Id:"), "{tag_block}");
}

#[test]
fn a_transformed_atom_over_a_derived_prior_orders_the_transform_trailer_first() {
    let mut builder = IrBuilder::new(provenance(SourceKind::Cvs));
    let step = builder.add_transform(brygge_ir::TransformRecord {
        name: "redact".to_string(),
        version: "1.0.0".to_string(),
        params: BTreeMap::new(),
        consumed: [0x11; 32],
    });
    let mut derived_params = BTreeMap::new();
    derived_params.insert("window_secs".to_string(), "180".to_string());
    let prior = brygge_ir::EpistemicStatus::Derived(Derivation {
        kind: DerivationKind::ReconstructedChangeset,
        by: "brygge-decode-cvs".to_string(),
        decoder_version: "0.5.0".to_string(),
        params: derived_params,
        confidence: None,
    });
    let blob = builder.add_blob(b"x\n".to_vec()).unwrap();
    let atom_id = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![brygge_ir::PathOp::Add {
                path: "f".to_string(),
                content: Content::blob(blob, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims::default(),
            source: source(SourceKind::Cvs, b"r1"),
            status: brygge_ir::EpistemicStatus::Transformed {
                step: step as u64,
                changed: vec![ChangedPart::Author],
                prior: Box::new(prior),
            },
        })
        .unwrap();
    builder
        .add_ref(RefRecord {
            name: "main".to_string(),
            kind: RefKind::Branch,
            target: atom_id,
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    let ir = builder.finish().unwrap();
    let mut out = Vec::new();
    write_stream(&ir, &[0u8; 32], &mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    let transformed_at = text
        .find("Brygge-Transformed: atom 0 redact 1.0.0 (author)")
        .unwrap();
    let derived_at = text
        .find("Brygge-Derived: atom reconstructed-changeset window_secs=180")
        .unwrap();
    assert!(transformed_at < derived_at, "{text}");
}

#[test]
fn a_transformed_branch_ref_puts_its_mark_in_the_summary_not_a_trailer() {
    let mut builder = IrBuilder::new(provenance(SourceKind::Hg));
    let step = builder.add_transform(brygge_ir::TransformRecord {
        name: "rename-branch".to_string(),
        version: "1.0.0".to_string(),
        params: BTreeMap::new(),
        consumed: [0x22; 32],
    });
    let blob = builder.add_blob(b"x\n".to_vec()).unwrap();
    let atom_id = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![brygge_ir::PathOp::Add {
                path: "f".to_string(),
                content: Content::blob(blob, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims::default(),
            source: source(SourceKind::Hg, b"r1"),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    builder
        .add_ref(RefRecord {
            name: "renamed".to_string(),
            kind: RefKind::Branch,
            target: atom_id,
            status: brygge_ir::EpistemicStatus::Transformed {
                step: step as u64,
                changed: vec![ChangedPart::Name],
                prior: Box::new(brygge_ir::EpistemicStatus::Stated),
            },
            source: None,
            annotation: None,
        })
        .unwrap();
    let ir = builder.finish().unwrap();
    let mut out = Vec::new();
    let summary = write_stream(&ir, &[0u8; 32], &mut out).unwrap();
    assert_eq!(summary.unplaced_ref_transforms.len(), 1);
    assert!(summary.unplaced_ref_transforms[0].contains("renamed"));
    let text = String::from_utf8(out).unwrap();
    assert!(
        !text.contains("Brygge-Transformed"),
        "a branch has no message to carry a trailer in: {text}"
    );
}

#[test]
fn supplied_and_altered_are_counted_in_the_summary() {
    let mut builder = IrBuilder::new(provenance(SourceKind::Git));
    let blob = builder.add_blob(b"x\n".to_vec()).unwrap();
    let atom_id = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![brygge_ir::PathOp::Add {
                path: "f".to_string(),
                content: Content::blob(blob, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims {
                author: Some(Identity {
                    name: Text {
                        bytes: vec![0xff, 0xfe],
                        encoding: None,
                    },
                    email: None,
                }),
                author_time: None,
                committer: None,
                commit_time: None,
                message: None,
            },
            source: source(SourceKind::Git, b"r1"),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    builder
        .add_ref(RefRecord {
            name: "main".to_string(),
            kind: RefKind::Branch,
            target: atom_id,
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    let ir = builder.finish().unwrap();
    let mut out = Vec::new();
    let summary = write_stream(&ir, &[0u8; 32], &mut out).unwrap();
    // Review 136 F-1: the identity itself is present (name non-UTF-8, altered); only the email is
    // missing, so the mark is "author-email", never the bare "author" (that would claim the whole
    // identity was invented).
    assert_eq!(summary.altered.get("author-name").copied(), Some(1));
    assert_eq!(summary.supplied.get("author-email").copied(), Some(1));
    assert_eq!(summary.supplied.get("author-time").copied(), Some(1));
    assert_eq!(summary.supplied.get("committer").copied(), Some(1));
    assert_eq!(summary.supplied.get("commit-time").copied(), Some(1));
}

// ---- reachability (review 136 F-2) -------------------------------------------------------------------

/// A one-atom IR with no refs at all.
fn ir_with_no_refs(kind: SourceKind) -> brygge_ir::Ir {
    let mut builder = IrBuilder::new(provenance(kind.clone()));
    let blob = builder.add_blob(b"x\n".to_vec()).unwrap();
    builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![brygge_ir::PathOp::Add {
                path: "f".to_string(),
                content: Content::blob(blob, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims::default(),
            source: source(kind, b"r1"),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    builder.finish().unwrap()
}

#[test]
fn zero_refs_is_refused_naming_the_svn_fix() {
    let ir = ir_with_no_refs(SourceKind::Svn);
    let refusals = preflight(&ir);
    assert_eq!(refusals.len(), 1);
    match &refusals[0] {
        Refusal::UnreachableAtoms { count, fix, .. } => {
            assert_eq!(*count, 1);
            assert!(fix.contains("--reconstruct-refs"), "{fix}");
        }
        other => panic!("expected UnreachableAtoms, got {other:?}"),
    }
}

#[test]
fn zero_refs_on_a_git_source_names_a_transform_not_reconstruct_refs() {
    let ir = ir_with_no_refs(SourceKind::Git);
    let refusals = preflight(&ir);
    assert_eq!(refusals.len(), 1);
    match &refusals[0] {
        Refusal::UnreachableAtoms { fix, .. } => {
            assert!(fix.contains("a ref was removed by a transform"), "{fix}");
            assert!(!fix.contains("--reconstruct-refs"), "{fix}");
        }
        other => panic!("expected UnreachableAtoms, got {other:?}"),
    }
}

#[test]
fn a_transform_drop_naming_a_ref_names_its_step_in_the_fix() {
    use brygge_ir::DropRecord;
    let mut builder = IrBuilder::new(provenance(SourceKind::Hg));
    let step = builder.add_transform(brygge_ir::TransformRecord {
        name: "prune-branches".to_string(),
        version: "1.0.0".to_string(),
        params: BTreeMap::new(),
        consumed: [0x33; 32],
    });
    let blob = builder.add_blob(b"x\n".to_vec()).unwrap();
    builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![brygge_ir::PathOp::Add {
                path: "f".to_string(),
                content: Content::blob(blob, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims::default(),
            source: source(SourceKind::Hg, b"r1"),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    builder.set_loss(brygge_ir::LossBoundary {
        dropped: vec![DropRecord {
            class: brygge_ir::LossClass::Transform,
            what: "ref branch pruned-branch".to_string(),
            reason: "no longer needed".to_string(),
            step: Some(step as u64),
        }],
    });
    let ir = builder.finish().unwrap();
    let refusals = preflight(&ir);
    assert_eq!(refusals.len(), 1);
    match &refusals[0] {
        Refusal::UnreachableAtoms { fix, .. } => {
            assert!(fix.contains(&format!("step {step}")), "{fix}");
        }
        other => panic!("expected UnreachableAtoms, got {other:?}"),
    }
}

#[test]
fn partial_reachability_names_only_the_unreachable_count() {
    // Two independent atoms (no parent relationship): one ref points at the first; the second is
    // unreachable. Confirms the walk does not treat "some ref exists" as "everything is reachable".
    let mut builder = IrBuilder::new(provenance(SourceKind::Git));
    let blob1 = builder.add_blob(b"one\n".to_vec()).unwrap();
    let reachable_id = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![brygge_ir::PathOp::Add {
                path: "a".to_string(),
                content: Content::blob(blob1, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims::default(),
            source: source(SourceKind::Git, b"r1"),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    let blob2 = builder.add_blob(b"two\n".to_vec()).unwrap();
    builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![brygge_ir::PathOp::Add {
                path: "b".to_string(),
                content: Content::blob(blob2, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims::default(),
            source: source(SourceKind::Git, b"r2"),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    builder
        .add_ref(RefRecord {
            name: "main".to_string(),
            kind: RefKind::Branch,
            target: reachable_id,
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    let ir = builder.finish().unwrap();
    let refusals = preflight(&ir);
    assert_eq!(refusals.len(), 1);
    match &refusals[0] {
        Refusal::UnreachableAtoms { count, .. } => assert_eq!(*count, 1),
        other => panic!("expected UnreachableAtoms, got {other:?}"),
    }
}

#[test]
fn full_reachability_through_a_parent_chain_passes() {
    // A ref targets only the tip; its parent must still count as reachable through the walk.
    let mut builder = IrBuilder::new(provenance(SourceKind::Git));
    let blob1 = builder.add_blob(b"one\n".to_vec()).unwrap();
    let parent_id = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![brygge_ir::PathOp::Add {
                path: "a".to_string(),
                content: Content::blob(blob1, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims::default(),
            source: source(SourceKind::Git, b"r1"),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    let blob2 = builder.add_blob(b"two\n".to_vec()).unwrap();
    let tip_id = builder
        .add_atom(AtomDraft {
            parents: vec![parent_id],
            ops: vec![brygge_ir::PathOp::Add {
                path: "b".to_string(),
                content: Content::blob(blob2, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims::default(),
            source: source(SourceKind::Git, b"r2"),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    builder
        .add_ref(RefRecord {
            name: "main".to_string(),
            kind: RefKind::Branch,
            target: tip_id,
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    let ir = builder.finish().unwrap();
    assert!(preflight(&ir).is_empty());
}
// ---- review 137 F-8: the reachability hint when refs do exist -----------------------------------------

#[test]
fn svn_with_refs_present_says_no_reconstructed_ref_reaches_not_reconstruct_refs() {
    // One ref exists (reachable_id), but a second, independent atom is not reachable from it: the hint
    // must not suggest --reconstruct-refs again (it was already effective enough to produce a ref).
    let mut builder = IrBuilder::new(provenance(SourceKind::Svn));
    let blob1 = builder.add_blob(b"one\n".to_vec()).unwrap();
    let reachable_id = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![brygge_ir::PathOp::Add {
                path: "a".to_string(),
                content: Content::blob(blob1, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims::default(),
            source: source(SourceKind::Svn, &1u64.to_be_bytes()),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    let blob2 = builder.add_blob(b"two\n".to_vec()).unwrap();
    builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![brygge_ir::PathOp::Add {
                path: "b".to_string(),
                content: Content::blob(blob2, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims::default(),
            source: source(SourceKind::Svn, &2u64.to_be_bytes()),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    builder
        .add_ref(RefRecord {
            name: "trunk".to_string(),
            kind: RefKind::Branch,
            target: reachable_id,
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    let ir = builder.finish().unwrap();
    let refusals = preflight(&ir);
    assert_eq!(refusals.len(), 1);
    match &refusals[0] {
        Refusal::UnreachableAtoms { fix, .. } => {
            assert!(fix.contains("no reconstructed ref reaches"), "{fix}");
            assert!(!fix.contains("--reconstruct-refs"), "{fix}");
        }
        other => panic!("expected UnreachableAtoms, got {other:?}"),
    }
}

#[test]
fn cvs_with_refs_present_says_no_reconstructed_ref_reaches_these_atoms() {
    let mut builder = IrBuilder::new(provenance(SourceKind::Cvs));
    let blob1 = builder.add_blob(b"one\n".to_vec()).unwrap();
    let reachable_id = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![brygge_ir::PathOp::Add {
                path: "a".to_string(),
                content: Content::blob(blob1, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims::default(),
            source: source(SourceKind::Cvs, b"a@1.1"),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    let blob2 = builder.add_blob(b"two\n".to_vec()).unwrap();
    builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![brygge_ir::PathOp::Add {
                path: "b".to_string(),
                content: Content::blob(blob2, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims::default(),
            source: source(SourceKind::Cvs, b"b@1.1"),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    builder
        .add_ref(RefRecord {
            name: "a-branch".to_string(),
            kind: RefKind::Branch,
            target: reachable_id,
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    let ir = builder.finish().unwrap();
    let refusals = preflight(&ir);
    assert_eq!(refusals.len(), 1);
    match &refusals[0] {
        Refusal::UnreachableAtoms { fix, .. } => {
            assert!(fix.contains("no reconstructed ref reaches"), "{fix}");
            assert!(!fix.contains("--reconstruct-refs"), "{fix}");
        }
        other => panic!("expected UnreachableAtoms, got {other:?}"),
    }
}
