//! Level 1 (RFC 019 D-6 §3): hand-built artifact/result pairs, one row of §2 broken at a time. Never
//! runs the real writer or a real `git` — `crates/brygge/tests/encode_git_check.rs` is the level-2
//! proof against a real `git fast-import`/`git replace`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;

use super::*;
use brygge_ir::{
    AtomDraft, BlobId, Content, ImportProvenance, IrBuilder, MetadataClaims, PathOp,
    SourceIdentity, Text, Time,
};

fn git_source(atom_id: &[u8]) -> SourceIdentity {
    SourceIdentity {
        kind: SourceKind::Git,
        repo_id: Vec::new(),
        atom_id: atom_id.to_vec(),
        signatures: Vec::new(),
        extras: Vec::new(),
    }
}

fn provenance() -> ImportProvenance {
    ImportProvenance {
        source: git_source(b"repo"),
        brygge_version: "0.0.0".to_string(),
        decoder: "test".to_string(),
        decoder_version: "0.0.0".to_string(),
        params: BTreeMap::new(),
    }
}

const DIGEST: [u8; 32] = [0x11; 32];
const ARTIFACT_ATOM_ID: [u8; 20] = [1; 20];
const RESULT_COMMIT_ID: [u8; 20] = [2; 20];

/// The label a matched-pair difference must carry (review 142 N-1): the artifact's own
/// `Brygge-Source-Id`, never the IR's internal `AtomId`, plus the result's own commit id.
fn expected_label() -> String {
    format!(
        "source {} (commit {})",
        trailers::to_hex(&ARTIFACT_ATOM_ID),
        trailers::to_hex(&RESULT_COMMIT_ID)
    )
}

fn trailer_block(source_id_hex: &str, digest_hex: &str) -> String {
    format!(
        "Brygge-Encoded: brygge {} from git\nBrygge-Source-Id: {source_id_hex}\nBrygge-Artifact: \
         sha256:{digest_hex}",
        env!("CARGO_PKG_VERSION")
    )
}

/// A one-atom, one-ref artifact: an explicit committer and commit time distinct from the author, so no
/// `Brygge-Supplied`/`Brygge-Altered` trailer appears in the baseline — each test below perturbs exactly
/// one thing away from this matching pair.
struct Baseline {
    artifact: Ir,
    blob: BlobId,
}

fn baseline_artifact() -> Baseline {
    let mut builder = IrBuilder::new(provenance());
    let blob = builder.add_blob(b"hello\n".to_vec()).unwrap();
    let atom_id = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![PathOp::Add {
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
                    seconds: 1_000,
                    offset_minutes: Some(0),
                }),
                committer: Some(Identity {
                    name: Text::utf8("C O Mitter"),
                    email: Some(Text::utf8("c@example.com")),
                }),
                commit_time: Some(Time {
                    seconds: 2_000,
                    offset_minutes: Some(60),
                }),
                message: Some(Text::utf8("hello")),
            },
            source: git_source(&ARTIFACT_ATOM_ID),
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
    let artifact = builder.finish().unwrap();
    Baseline { artifact, blob }
}

/// A matching result: one commit whose `Brygge-Source-Id` names the artifact's one atom, with the exact
/// identity/time/tree/message the artifact's writer rules dictate, and the ref listing D-3 maps to.
fn matching_result(base: &Baseline) -> (Ir, Vec<GitRef>) {
    let mut builder = IrBuilder::new(provenance());
    let result_blob = builder.add_blob(b"hello\n".to_vec()).unwrap();
    assert_eq!(result_blob, base.blob, "same content, same address");
    let source_id_hex = trailers::to_hex(&ARTIFACT_ATOM_ID);
    let digest_hex = trailers::to_hex(&DIGEST);
    let message = format!("hello\n\n{}", trailer_block(&source_id_hex, &digest_hex));
    let commit_id = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![PathOp::Add {
                path: "f.txt".to_string(),
                content: Content::blob(result_blob, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims {
                author: Some(Identity {
                    name: Text::utf8("A U Thor"),
                    email: Some(Text::utf8("a@example.com")),
                }),
                author_time: Some(Time {
                    seconds: 1_000,
                    offset_minutes: Some(0),
                }),
                committer: Some(Identity {
                    name: Text::utf8("C O Mitter"),
                    email: Some(Text::utf8("c@example.com")),
                }),
                commit_time: Some(Time {
                    seconds: 2_000,
                    offset_minutes: Some(60),
                }),
                message: Some(Text::utf8(&message)),
            },
            source: git_source(&RESULT_COMMIT_ID),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    builder
        .add_ref(RefRecord {
            name: "main".to_string(),
            kind: RefKind::Branch,
            target: commit_id,
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    let result = builder.finish().unwrap();
    let refs = vec![
        GitRef {
            name: "refs/heads/main".to_string(),
            target: GitRefTarget::Oid(RESULT_COMMIT_ID.to_vec()),
        },
        GitRef {
            name: "HEAD".to_string(),
            target: GitRefTarget::Symbolic("refs/heads/main".to_string()),
        },
    ];
    (result, refs)
}

fn run(base: &Baseline, result: &Ir, refs: &[GitRef]) -> Comparison {
    compare_git_target(&base.artifact, &DIGEST, result, refs, "refs/heads/main")
}

#[test]
fn a_matching_pair_corresponds() {
    let base = baseline_artifact();
    let (result, refs) = matching_result(&base);
    assert!(matches!(
        run(&base, &result, &refs),
        Comparison::Corresponds
    ));
}

#[test]
fn two_artifact_atoms_sharing_one_source_id_are_not_checked() {
    let mut builder = IrBuilder::new(provenance());
    let blob = builder.add_blob(b"x".to_vec()).unwrap();
    let draft = |parents: Vec<AtomId>| AtomDraft {
        parents,
        ops: vec![PathOp::Add {
            path: "x".to_string(),
            content: Content::blob(blob, 0o100_644),
            status: brygge_ir::EpistemicStatus::Stated,
        }],
        copies: Vec::new(),
        metadata: MetadataClaims::default(),
        source: git_source(b"same"),
        status: brygge_ir::EpistemicStatus::Stated,
    };
    let first = builder.add_atom(draft(Vec::new())).unwrap();
    builder.add_atom(draft(vec![first])).unwrap();
    let artifact = builder.finish().unwrap();
    match compare_git_target(&artifact, &DIGEST, &artifact, &[], "refs/heads/main") {
        Comparison::NotChecked(reason) => {
            assert!(reason.contains("share one source id"), "{reason}");
        }
        other => panic!("expected NotChecked, got {other:?}"),
    }
}

fn perturbed_atom<F: FnOnce(&mut AtomDraft)>(
    base: &Baseline,
    digest_hex: &str,
    f: F,
) -> (Ir, Vec<GitRef>) {
    let mut builder = IrBuilder::new(provenance());
    let result_blob = builder.add_blob(b"hello\n".to_vec()).unwrap();
    let source_id_hex = trailers::to_hex(&ARTIFACT_ATOM_ID);
    let message = format!("hello\n\n{}", trailer_block(&source_id_hex, digest_hex));
    let mut draft = AtomDraft {
        parents: Vec::new(),
        ops: vec![PathOp::Add {
            path: "f.txt".to_string(),
            content: Content::blob(result_blob, 0o100_644),
            status: brygge_ir::EpistemicStatus::Stated,
        }],
        copies: Vec::new(),
        metadata: MetadataClaims {
            author: Some(Identity {
                name: Text::utf8("A U Thor"),
                email: Some(Text::utf8("a@example.com")),
            }),
            author_time: Some(Time {
                seconds: 1_000,
                offset_minutes: Some(0),
            }),
            committer: Some(Identity {
                name: Text::utf8("C O Mitter"),
                email: Some(Text::utf8("c@example.com")),
            }),
            commit_time: Some(Time {
                seconds: 2_000,
                offset_minutes: Some(60),
            }),
            message: Some(Text::utf8(&message)),
        },
        source: git_source(&RESULT_COMMIT_ID),
        status: brygge_ir::EpistemicStatus::Stated,
    };
    f(&mut draft);
    let commit_id = builder.add_atom(draft).unwrap();
    builder
        .add_ref(RefRecord {
            name: "main".to_string(),
            kind: RefKind::Branch,
            target: commit_id,
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    let result = builder.finish().unwrap();
    let refs = vec![
        GitRef {
            name: "refs/heads/main".to_string(),
            target: GitRefTarget::Oid(RESULT_COMMIT_ID.to_vec()),
        },
        GitRef {
            name: "HEAD".to_string(),
            target: GitRefTarget::Symbolic("refs/heads/main".to_string()),
        },
    ];
    let _ = base;
    (result, refs)
}

fn assert_one_diff_containing(base: &Baseline, result: &Ir, refs: &[GitRef], needle: &str) {
    match run(base, result, refs) {
        Comparison::DoesNotCorrespond(diffs) => {
            assert!(
                diffs.iter().any(|d| d.contains(needle)),
                "expected a diff containing {needle:?}, got {diffs:?}"
            );
        }
        other => panic!("expected DoesNotCorrespond, got {other:?}"),
    }
}

#[test]
fn a_different_tree_is_caught() {
    let base = baseline_artifact();
    let digest_hex = trailers::to_hex(&DIGEST);
    let mut builder = IrBuilder::new(provenance());
    let different_blob = builder.add_blob(b"goodbye\n".to_vec()).unwrap();
    let (result, refs) = perturbed_atom(&base, &digest_hex, |draft| {
        draft.ops = vec![PathOp::Add {
            path: "f.txt".to_string(),
            content: Content::blob(different_blob, 0o100_644),
            status: brygge_ir::EpistemicStatus::Stated,
        }];
    });
    assert_one_diff_containing(
        &base,
        &result,
        &refs,
        &format!("{}: the resulting tree differs", expected_label()),
    );
}

#[test]
fn a_different_author_is_caught() {
    let base = baseline_artifact();
    let digest_hex = trailers::to_hex(&DIGEST);
    let (result, refs) = perturbed_atom(&base, &digest_hex, |draft| {
        draft.metadata.author = Some(Identity {
            name: Text::utf8("Someone Else"),
            email: Some(Text::utf8("else@example.com")),
        });
    });
    assert_one_diff_containing(
        &base,
        &result,
        &refs,
        &format!("{}: author differs", expected_label()),
    );
}

#[test]
fn a_different_committer_is_caught() {
    let base = baseline_artifact();
    let digest_hex = trailers::to_hex(&DIGEST);
    let (result, refs) = perturbed_atom(&base, &digest_hex, |draft| {
        draft.metadata.committer = Some(Identity {
            name: Text::utf8("Someone Else"),
            email: Some(Text::utf8("else@example.com")),
        });
    });
    assert_one_diff_containing(
        &base,
        &result,
        &refs,
        &format!("{}: committer differs", expected_label()),
    );
}

#[test]
fn a_different_message_is_caught() {
    let base = baseline_artifact();
    let digest_hex = trailers::to_hex(&DIGEST);
    let source_id_hex = trailers::to_hex(&ARTIFACT_ATOM_ID);
    let (result, refs) = perturbed_atom(&base, &digest_hex, |draft| {
        let wrong = format!(
            "not the source message\n\n{}",
            trailer_block(&source_id_hex, &digest_hex)
        );
        draft.metadata.message = Some(Text::utf8(&wrong));
    });
    assert_one_diff_containing(
        &base,
        &result,
        &refs,
        &format!("{}: message (with its trailers) differs", expected_label()),
    );
}

#[test]
fn a_result_commit_with_no_trailer_is_a_difference() {
    let base = baseline_artifact();
    let (result, refs) = perturbed_atom(&base, "unused", |draft| {
        draft.metadata.message = Some(Text::utf8("hello\n"));
    });
    assert_one_diff_containing(
        &base,
        &result,
        &refs,
        &format!(
            "commit {}: has no Brygge-Source-Id trailer",
            trailers::to_hex(&RESULT_COMMIT_ID)
        ),
    );
}

#[test]
fn a_result_commit_naming_an_unknown_atom_is_a_difference() {
    let base = baseline_artifact();
    let digest_hex = trailers::to_hex(&DIGEST);
    let (result, refs) = perturbed_atom(&base, &digest_hex, |draft| {
        let message = format!(
            "hello\n\n{}",
            trailer_block(&trailers::to_hex(&[9; 20]), &digest_hex)
        );
        draft.metadata.message = Some(Text::utf8(&message));
    });
    assert_one_diff_containing(
        &base,
        &result,
        &refs,
        &format!(
            "commit {}: its Brygge-Source-Id ",
            trailers::to_hex(&RESULT_COMMIT_ID)
        ),
    );
    assert_one_diff_containing(&base, &result, &refs, "names no atom in the artifact");
}

const DEAD_ARTIFACT_ATOM_ID: [u8; 20] = [9; 20];
const DEAD_RESULT_COMMIT_ID: [u8; 20] = [8; 20];

/// An artifact with a live atom (on `refs/heads/main`) and a dead/earlier-life one (on an `Other` ref,
/// `dead@r1`) — RFC 019 Amendment A's `--branch-lines` shape: a line whose history Git would otherwise
/// garbage-collect, kept alive under `refs/brygge/other/`.
fn baseline_with_a_dead_line() -> Baseline {
    let mut builder = IrBuilder::new(provenance());
    let blob = builder.add_blob(b"hello\n".to_vec()).unwrap();
    let live_atom = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![PathOp::Add {
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
                    seconds: 1_000,
                    offset_minutes: Some(0),
                }),
                committer: Some(Identity {
                    name: Text::utf8("C O Mitter"),
                    email: Some(Text::utf8("c@example.com")),
                }),
                commit_time: Some(Time {
                    seconds: 2_000,
                    offset_minutes: Some(60),
                }),
                message: Some(Text::utf8("hello")),
            },
            source: git_source(&ARTIFACT_ATOM_ID),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    let dead_atom = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![PathOp::Add {
                path: "dead.txt".to_string(),
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
                    seconds: 1_000,
                    offset_minutes: Some(0),
                }),
                committer: Some(Identity {
                    name: Text::utf8("C O Mitter"),
                    email: Some(Text::utf8("c@example.com")),
                }),
                commit_time: Some(Time {
                    seconds: 2_000,
                    offset_minutes: Some(60),
                }),
                message: Some(Text::utf8("gone but not forgotten")),
            },
            source: git_source(&DEAD_ARTIFACT_ATOM_ID),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    builder
        .add_ref(RefRecord {
            name: "main".to_string(),
            kind: RefKind::Branch,
            target: live_atom,
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    builder
        .add_ref(RefRecord {
            name: "dead@r1".to_string(),
            kind: RefKind::Other("deleted-branch".to_string()),
            target: dead_atom,
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    let artifact = builder.finish().unwrap();
    Baseline { artifact, blob }
}

/// A result matching `baseline_with_a_dead_line()` exactly, if `dead_tree_matches`; otherwise the dead
/// line's own commit gets a different tree (review 147 F-1: this must be *caught*, not exempted).
/// Review 147 F-1: `brygge_decode_git`'s `carry_extra_namespaces` makes this decode really see the dead
/// line's commit (a real repository's `refs/brygge/other/*`), so both commits appear in `result.atoms`
/// here exactly as a real check run would get them — this test proves `compare_git_target`'s own logic,
/// not the decoder wiring (level 2 proves that, for real, against a real `svnadmin`/`git` repository).
fn result_with_a_dead_line(base: &Baseline, dead_tree_matches: bool) -> (Ir, Vec<GitRef>) {
    let mut builder = IrBuilder::new(provenance());
    let live_blob = builder.add_blob(b"hello\n".to_vec()).unwrap();
    assert_eq!(live_blob, base.blob);
    let digest_hex = trailers::to_hex(&DIGEST);
    let live_source_hex = trailers::to_hex(&ARTIFACT_ATOM_ID);
    let live_message = format!("hello\n\n{}", trailer_block(&live_source_hex, &digest_hex));
    let live_commit = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![PathOp::Add {
                path: "f.txt".to_string(),
                content: Content::blob(live_blob, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims {
                author: Some(Identity {
                    name: Text::utf8("A U Thor"),
                    email: Some(Text::utf8("a@example.com")),
                }),
                author_time: Some(Time {
                    seconds: 1_000,
                    offset_minutes: Some(0),
                }),
                committer: Some(Identity {
                    name: Text::utf8("C O Mitter"),
                    email: Some(Text::utf8("c@example.com")),
                }),
                commit_time: Some(Time {
                    seconds: 2_000,
                    offset_minutes: Some(60),
                }),
                message: Some(Text::utf8(&live_message)),
            },
            source: git_source(&RESULT_COMMIT_ID),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();

    let dead_path = if dead_tree_matches {
        "dead.txt"
    } else {
        "tampered.txt"
    };
    let dead_source_hex = trailers::to_hex(&DEAD_ARTIFACT_ATOM_ID);
    let dead_message = format!(
        "gone but not forgotten\n\n{}",
        trailer_block(&dead_source_hex, &digest_hex)
    );
    let dead_commit = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![PathOp::Add {
                path: dead_path.to_string(),
                content: Content::blob(live_blob, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims {
                author: Some(Identity {
                    name: Text::utf8("A U Thor"),
                    email: Some(Text::utf8("a@example.com")),
                }),
                author_time: Some(Time {
                    seconds: 1_000,
                    offset_minutes: Some(0),
                }),
                committer: Some(Identity {
                    name: Text::utf8("C O Mitter"),
                    email: Some(Text::utf8("c@example.com")),
                }),
                commit_time: Some(Time {
                    seconds: 2_000,
                    offset_minutes: Some(60),
                }),
                message: Some(Text::utf8(&dead_message)),
            },
            source: git_source(&DEAD_RESULT_COMMIT_ID),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();

    builder
        .add_ref(RefRecord {
            name: "main".to_string(),
            kind: RefKind::Branch,
            target: live_commit,
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    builder
        .add_ref(RefRecord {
            name: "dead@r1".to_string(),
            kind: RefKind::Other("carry-extra-namespaces".to_string()),
            target: dead_commit,
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    let result = builder.finish().unwrap();
    let refs = vec![
        GitRef {
            name: "refs/heads/main".to_string(),
            target: GitRefTarget::Oid(RESULT_COMMIT_ID.to_vec()),
        },
        GitRef {
            name: "refs/brygge/other/dead@r1".to_string(),
            target: GitRefTarget::Oid(DEAD_RESULT_COMMIT_ID.to_vec()),
        },
        GitRef {
            name: "HEAD".to_string(),
            target: GitRefTarget::Symbolic("refs/heads/main".to_string()),
        },
    ];
    (result, refs)
}

/// Review 147 F-1: a dead line's own commit, decoded for real (via `carry_extra_namespaces`), is
/// compared just like any other commit — not exempted.
#[test]
fn a_dead_line_with_a_matching_commit_corresponds() {
    let base = baseline_with_a_dead_line();
    let (result, refs) = result_with_a_dead_line(&base, true);
    match run(&base, &result, &refs) {
        Comparison::Corresponds => {}
        other => panic!("expected Corresponds, got {other:?}"),
    }
}

/// The other half of F-1: a *wrong* tree on the dead line's own commit must be caught, exactly as it
/// would be for a live one — this is the gap `e1e4c0e`'s exemption approach left open.
#[test]
fn a_dead_line_with_a_different_tree_does_not_correspond() {
    let base = baseline_with_a_dead_line();
    let (result, refs) = result_with_a_dead_line(&base, false);
    match run(&base, &result, &refs) {
        Comparison::DoesNotCorrespond(diffs) => {
            assert!(
                diffs.iter().any(|d| d.contains("tree differs")),
                "{diffs:?}"
            );
        }
        other => panic!("expected DoesNotCorrespond, got {other:?}"),
    }
}

#[test]
fn an_artifact_atom_with_no_matching_commit_is_a_difference() {
    let base = baseline_artifact();
    // An empty result (no commits, no refs): the artifact's one atom is never named.
    let empty = IrBuilder::new(provenance()).finish().unwrap();
    assert_one_diff_containing(
        &base,
        &empty,
        &[],
        &format!(
            "source {}: no commit in the result names it",
            trailers::to_hex(&ARTIFACT_ATOM_ID)
        ),
    );
}

#[test]
fn two_result_commits_claiming_the_same_artifact_atom_are_named_by_their_own_commit_ids() {
    // Review 142 N-1: this row used to name the two result commits by their internal `AtomId`s. It must
    // instead name the claimed artifact atom by its source id, and each claimant by its own real commit
    // id (here, each result atom's own `source.atom_id` — the two are deliberately distinct from each
    // other and from `RESULT_COMMIT_ID`, so a passing assertion cannot be satisfied by an unrelated id).
    let base = baseline_artifact();
    let digest_hex = trailers::to_hex(&DIGEST);
    let source_id_hex = trailers::to_hex(&ARTIFACT_ATOM_ID);
    let mut builder = IrBuilder::new(provenance());
    let blob_a = builder.add_blob(b"hello\n".to_vec()).unwrap();
    let message = format!("hello\n\n{}", trailer_block(&source_id_hex, &digest_hex));
    let claimant = |blob: BlobId, commit_atom_id: &[u8; 20]| AtomDraft {
        parents: Vec::new(),
        ops: vec![PathOp::Add {
            path: "f.txt".to_string(),
            content: Content::blob(blob, 0o100_644),
            status: brygge_ir::EpistemicStatus::Stated,
        }],
        copies: Vec::new(),
        metadata: MetadataClaims {
            message: Some(Text::utf8(&message)),
            ..MetadataClaims::default()
        },
        source: git_source(commit_atom_id),
        status: brygge_ir::EpistemicStatus::Stated,
    };
    const FIRST_CLAIMANT: [u8; 20] = [7; 20];
    const SECOND_CLAIMANT: [u8; 20] = [8; 20];
    builder.add_atom(claimant(blob_a, &FIRST_CLAIMANT)).unwrap();
    let blob_b = builder.add_blob(b"hello\n".to_vec()).unwrap();
    builder
        .add_atom(claimant(blob_b, &SECOND_CLAIMANT))
        .unwrap();
    let result = builder.finish().unwrap();
    assert_one_diff_containing(
        &base,
        &result,
        &[],
        &format!(
            "source {source_id_hex}: two result commits both claim it, commit {} and commit {}",
            trailers::to_hex(&FIRST_CLAIMANT),
            trailers::to_hex(&SECOND_CLAIMANT),
        ),
    );
}

#[test]
fn a_missing_ref_is_caught() {
    let base = baseline_artifact();
    let (result, _refs) = matching_result(&base);
    let refs = vec![GitRef {
        name: "HEAD".to_string(),
        target: GitRefTarget::Symbolic("refs/heads/main".to_string()),
    }];
    assert_one_diff_containing(&base, &result, &refs, "missing from the result");
}

#[test]
fn an_extra_ref_is_nothing_extra() {
    let base = baseline_artifact();
    let (result, mut refs) = matching_result(&base);
    refs.push(GitRef {
        name: "refs/heads/extra".to_string(),
        target: GitRefTarget::Oid(RESULT_COMMIT_ID.to_vec()),
    });
    assert_one_diff_containing(&base, &result, &refs, "nothing extra");
}

#[test]
fn head_naming_the_wrong_branch_is_caught() {
    let base = baseline_artifact();
    let (result, mut refs) = matching_result(&base);
    for r in &mut refs {
        if r.name == "HEAD" {
            r.target = GitRefTarget::Symbolic("refs/heads/other".to_string());
        }
    }
    assert_one_diff_containing(&base, &result, &refs, "HEAD");
}

#[test]
fn a_different_parent_is_caught() {
    let mut builder = IrBuilder::new(provenance());
    let blob = builder.add_blob(b"a\n".to_vec()).unwrap();
    let root = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![PathOp::Add {
                path: "f".to_string(),
                content: Content::blob(blob, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims::default(),
            source: git_source(&[1u8; 20]),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    let _other_root = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: Vec::new(),
            copies: Vec::new(),
            metadata: MetadataClaims::default(),
            source: git_source(&[2u8; 20]),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    let blob2 = builder.add_blob(b"b\n".to_vec()).unwrap();
    let child = builder
        .add_atom(AtomDraft {
            parents: vec![root],
            ops: vec![PathOp::Modify {
                path: "f".to_string(),
                content: Content::blob(blob2, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims::default(),
            source: git_source(&[3u8; 20]),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    builder
        .add_ref(RefRecord {
            name: "main".to_string(),
            kind: RefKind::Branch,
            target: child,
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    let artifact = builder.finish().unwrap();
    let digest = [0x22; 32];
    let digest_hex = trailers::to_hex(&digest);

    // A matching result, except the child's result commit is parented on `other-root` instead of `root`.
    let mut rb = IrBuilder::new(provenance());
    let r_blob = rb.add_blob(b"a\n".to_vec()).unwrap();
    let message_for =
        |id: &[u8; 20]| format!("\n\n{}", trailer_block(&trailers::to_hex(id), &digest_hex));
    let _r_root = rb
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![PathOp::Add {
                path: "f".to_string(),
                content: Content::blob(r_blob, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims {
                message: Some(Text::utf8(&message_for(&[1u8; 20]))),
                ..MetadataClaims::default()
            },
            source: git_source(&[1u8; 20]),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    let r_other_root = rb
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: Vec::new(),
            copies: Vec::new(),
            metadata: MetadataClaims {
                message: Some(Text::utf8(&message_for(&[2u8; 20]))),
                ..MetadataClaims::default()
            },
            source: git_source(&[2u8; 20]),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    let r_blob2 = rb.add_blob(b"b\n".to_vec()).unwrap();
    let r_child = rb
        .add_atom(AtomDraft {
            // Wrong: parented on the other root, not `r_root`.
            parents: vec![r_other_root],
            ops: vec![PathOp::Modify {
                path: "f".to_string(),
                content: Content::blob(r_blob2, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims {
                message: Some(Text::utf8(&message_for(&[3u8; 20]))),
                ..MetadataClaims::default()
            },
            source: git_source(&[3u8; 20]),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    rb.add_ref(RefRecord {
        name: "main".to_string(),
        kind: RefKind::Branch,
        target: r_child,
        status: brygge_ir::EpistemicStatus::Stated,
        source: None,
        annotation: None,
    })
    .unwrap();
    let result = rb.finish().unwrap();
    let refs = vec![
        GitRef {
            name: "refs/heads/main".to_string(),
            target: GitRefTarget::Oid([3u8; 20].to_vec()),
        },
        GitRef {
            name: "HEAD".to_string(),
            target: GitRefTarget::Symbolic("refs/heads/main".to_string()),
        },
    ];
    match compare_git_target(&artifact, &digest, &result, &refs, "refs/heads/main") {
        Comparison::DoesNotCorrespond(diffs) => {
            let expected = format!(
                "source {} (commit {}): parents differ",
                trailers::to_hex(&[3u8; 20]),
                trailers::to_hex(&[3u8; 20])
            );
            assert!(diffs.iter().any(|d| d.contains(&expected)), "{diffs:?}");
        }
        other => panic!("expected DoesNotCorrespond, got {other:?}"),
    }
}

/// Regression (found on the real fish-shell corpus, batch G2): a shared parent used as the *first*
/// parent by one child and as a *merge* (non-first) parent by another child, where the merge child's
/// own canonical order comes first — the shared parent's tree state must survive to reach the later,
/// linear child. Source ids are chosen so the builder's own canonical (topological, source-id-tiebroken)
/// order processes the merge child (`b_merge`) before the linear one (`z_linear`): `C` and `X` are both
/// roots (`C` sorts first); once `X` is processed, both children become ready in the same pass, and
/// `"b_merge" < "z_linear"` picks the merge child first — exactly the order that exposed the eviction
/// bug (decrementing `X`'s use-count on a merge parent nobody's state read, not only on a first parent).
#[test]
fn a_shared_parent_used_as_both_first_and_merge_survives_to_its_linear_child() {
    let mut builder = IrBuilder::new(provenance());
    let blob_c = builder.add_blob(b"c\n".to_vec()).unwrap();
    let c = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![PathOp::Add {
                path: "c.txt".to_string(),
                content: Content::blob(blob_c, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims::default(),
            source: git_source(&[1u8; 20]),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    let blob_x = builder.add_blob(b"x\n".to_vec()).unwrap();
    let x = builder
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![PathOp::Add {
                path: "x.txt".to_string(),
                content: Content::blob(blob_x, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims::default(),
            source: git_source(&[2u8; 20]),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    let blob_b = builder.add_blob(b"b\n".to_vec()).unwrap();
    let b_merge = builder
        .add_atom(AtomDraft {
            parents: vec![c, x],
            ops: vec![PathOp::Add {
                path: "b.txt".to_string(),
                content: Content::blob(blob_b, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims::default(),
            source: git_source(&[3u8; 20]),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    let blob_z = builder.add_blob(b"z\n".to_vec()).unwrap();
    let z_linear = builder
        .add_atom(AtomDraft {
            parents: vec![x],
            ops: vec![PathOp::Add {
                path: "z.txt".to_string(),
                content: Content::blob(blob_z, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims::default(),
            source: git_source(&[4u8; 20]),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    builder
        .add_ref(RefRecord {
            name: "merge-side".to_string(),
            kind: RefKind::Branch,
            target: b_merge,
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    builder
        .add_ref(RefRecord {
            name: "linear-side".to_string(),
            kind: RefKind::Branch,
            target: z_linear,
            status: brygge_ir::EpistemicStatus::Stated,
            source: None,
            annotation: None,
        })
        .unwrap();
    let artifact = builder.finish().unwrap();

    // Confirm the canonical order actually puts the merge child before the linear one — otherwise this
    // test would not exercise the bug at all, regardless of what it asserts.
    let order: Vec<&[u8]> = artifact
        .atoms
        .iter()
        .map(|a| a.source.atom_id.as_slice())
        .collect();
    let b_pos = order.iter().position(|id| *id == [3u8; 20]).unwrap();
    let z_pos = order.iter().position(|id| *id == [4u8; 20]).unwrap();
    assert!(
        b_pos < z_pos,
        "test setup assumption failed: the merge child must be canonically ordered before the linear \
         child for this to exercise the eviction bug; got order {order:?}"
    );

    // A correct, hand-built result: each atom's tree state exactly as the artifact's own ops compute it
    // (so this asserts `Corresponds`, never tests the matching logic itself — that's `check/tests.rs`'s
    // other tests' job).
    let digest = [0x33; 32];
    let digest_hex = trailers::to_hex(&digest);
    let mut rb = IrBuilder::new(provenance());
    // Every atom here has `MetadataClaims::default()` (no author, time, committer or commit time
    // stated at all), so `write_stream` supplies all five and marks them — the trailer block must say
    // so too, or `expected_message`'s own re-render (which also starts from "nothing stated") will not
    // match this hand-built one.
    let label = |id: &[u8]| {
        format!(
            "\n\nBrygge-Encoded: brygge {} from git\nBrygge-Source-Id: {}\nBrygge-Supplied: author, \
             author-time, author-time-zone, committer, commit-time\nBrygge-Artifact: sha256:{digest_hex}",
            env!("CARGO_PKG_VERSION"),
            trailers::to_hex(id)
        )
    };
    let r_blob_c = rb.add_blob(b"c\n".to_vec()).unwrap();
    let r_c = rb
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![PathOp::Add {
                path: "c.txt".to_string(),
                content: Content::blob(r_blob_c, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims {
                author: Some(Identity {
                    name: Text::utf8("unknown"),
                    email: Some(Text::utf8("")),
                }),
                committer: Some(Identity {
                    name: Text::utf8("unknown"),
                    email: Some(Text::utf8("")),
                }),
                message: Some(Text::utf8(&label(&[1u8; 20]))),
                ..MetadataClaims::default()
            },
            source: git_source(&[1u8; 20]),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    let r_blob_x = rb.add_blob(b"x\n".to_vec()).unwrap();
    let r_x = rb
        .add_atom(AtomDraft {
            parents: Vec::new(),
            ops: vec![PathOp::Add {
                path: "x.txt".to_string(),
                content: Content::blob(r_blob_x, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims {
                author: Some(Identity {
                    name: Text::utf8("unknown"),
                    email: Some(Text::utf8("")),
                }),
                committer: Some(Identity {
                    name: Text::utf8("unknown"),
                    email: Some(Text::utf8("")),
                }),
                message: Some(Text::utf8(&label(&[2u8; 20]))),
                ..MetadataClaims::default()
            },
            source: git_source(&[2u8; 20]),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    let r_blob_b = rb.add_blob(b"b\n".to_vec()).unwrap();
    let r_b = rb
        .add_atom(AtomDraft {
            parents: vec![r_c, r_x],
            ops: vec![PathOp::Add {
                path: "b.txt".to_string(),
                content: Content::blob(r_blob_b, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims {
                author: Some(Identity {
                    name: Text::utf8("unknown"),
                    email: Some(Text::utf8("")),
                }),
                committer: Some(Identity {
                    name: Text::utf8("unknown"),
                    email: Some(Text::utf8("")),
                }),
                message: Some(Text::utf8(&label(&[3u8; 20]))),
                ..MetadataClaims::default()
            },
            // Deliberately a *different* raw source id than the artifact's own `b_merge` atom
            // ([3u8; 20]) — only the trailer's `Brygge-Source-Id` (the `label` above) determines which
            // artifact atom this matches; the raw id here only drives *this* result Ir's own canonical
            // order. Swapped with `r_z`'s below so the result side orders its linear child before its
            // merge child — the opposite of the artifact's own order — which is exactly what makes this
            // test able to tell the two sides' tree-state computations apart: if the eviction bug were
            // present, it would corrupt whichever side processes the merge child first, and the two
            // sides' (now differently-timed) corruptions would no longer agree with each other.
            source: git_source(&[4u8; 20]),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    let r_blob_z = rb.add_blob(b"z\n".to_vec()).unwrap();
    let r_z = rb
        .add_atom(AtomDraft {
            // The real, correct state after z_linear must include x.txt (from its parent x) plus
            // z.txt — exactly what the eviction bug would have dropped (x.txt), had x's ancestor
            // entry been evicted early by b_merge's own processing.
            parents: vec![r_x],
            ops: vec![PathOp::Add {
                path: "z.txt".to_string(),
                content: Content::blob(r_blob_z, 0o100_644),
                status: brygge_ir::EpistemicStatus::Stated,
            }],
            copies: Vec::new(),
            metadata: MetadataClaims {
                author: Some(Identity {
                    name: Text::utf8("unknown"),
                    email: Some(Text::utf8("")),
                }),
                committer: Some(Identity {
                    name: Text::utf8("unknown"),
                    email: Some(Text::utf8("")),
                }),
                message: Some(Text::utf8(&label(&[4u8; 20]))),
                ..MetadataClaims::default()
            },
            source: git_source(&[3u8; 20]),
            status: brygge_ir::EpistemicStatus::Stated,
        })
        .unwrap();
    rb.add_ref(RefRecord {
        name: "merge-side".to_string(),
        kind: RefKind::Branch,
        target: r_b,
        status: brygge_ir::EpistemicStatus::Stated,
        source: None,
        annotation: None,
    })
    .unwrap();
    rb.add_ref(RefRecord {
        name: "linear-side".to_string(),
        kind: RefKind::Branch,
        target: r_z,
        status: brygge_ir::EpistemicStatus::Stated,
        source: None,
        annotation: None,
    })
    .unwrap();
    let result = rb.finish().unwrap();
    let refs = vec![
        GitRef {
            // `r_b`'s own raw source id is [4u8; 20] now (swapped with `r_z`'s, above).
            name: "refs/heads/merge-side".to_string(),
            target: GitRefTarget::Oid([4u8; 20].to_vec()),
        },
        GitRef {
            // `r_z`'s own raw source id is [3u8; 20] now, for the same reason.
            name: "refs/heads/linear-side".to_string(),
            target: GitRefTarget::Oid([3u8; 20].to_vec()),
        },
        GitRef {
            name: "HEAD".to_string(),
            target: GitRefTarget::Symbolic("refs/heads/merge-side".to_string()),
        },
    ];
    match compare_git_target(&artifact, &digest, &result, &refs, "refs/heads/merge-side") {
        Comparison::Corresponds => {}
        other => panic!(
            "expected Corresponds (the eviction bug would wrongly report z_linear's tree as missing \
             x.txt), got {other:?}"
        ),
    }
}
