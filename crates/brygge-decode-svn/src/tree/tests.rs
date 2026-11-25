//! Unit tests for the running tree model: directory-copy expansion and subtree deletion.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::HashMap;

use brygge_ir::builder::IrBuilder;
use brygge_ir::model::{AtomId, ImportProvenance, PathOp, SourceIdentity, SourceKind};

use super::{Budget, FileEntry, Tree, apply_revision};
use crate::dumpstream::{Checksums, NodeAction, NodeKind, NodeRecord, PropBlock, TextBody};
use crate::source::Limits;

fn builder() -> IrBuilder {
    IrBuilder::new(ImportProvenance {
        source: SourceIdentity {
            kind: SourceKind::Svn,
            repo_id: Vec::new(),
            atom_id: Vec::new(),
            signatures: Vec::new(),
            extras: Vec::new(),
        },
        brygge_version: "test".to_string(),
        decoder: "test".to_string(),
        decoder_version: "test".to_string(),
        params: std::collections::BTreeMap::new(),
    })
}

fn file_node(path: &str, action: NodeAction, text: Option<&[u8]>) -> NodeRecord {
    NodeRecord {
        path: path.to_string(),
        kind: Some(NodeKind::File),
        action,
        copyfrom: None,
        props: Some(PropBlock::Full(Vec::new())),
        text: text.map(|t| TextBody::Full(t.to_vec())),
        checksums: Checksums::default(),
    }
}

fn dir_copy(path: &str, from_rev: u64, from_path: &str) -> NodeRecord {
    NodeRecord {
        path: path.to_string(),
        kind: Some(NodeKind::Dir),
        action: NodeAction::Add,
        copyfrom: Some((from_rev, from_path.to_string())),
        props: None,
        text: None,
        checksums: Checksums::default(),
    }
}

fn file_replace_with_copy(path: &str, from_rev: u64, from_path: &str) -> NodeRecord {
    NodeRecord {
        path: path.to_string(),
        kind: Some(NodeKind::File),
        action: NodeAction::Replace,
        copyfrom: Some((from_rev, from_path.to_string())),
        props: Some(PropBlock::Full(Vec::new())),
        text: None,
        checksums: Checksums::default(),
    }
}

fn added_paths(ops: &[PathOp]) -> Vec<String> {
    ops.iter()
        .filter_map(|o| match o {
            PathOp::Add { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_file_add_produces_an_add_op_and_populates_the_tree() {
    let mut b = builder();
    let kept = HashMap::new();
    let applied = apply_revision(
        &Tree::new(),
        vec![file_node("trunk/a.txt", NodeAction::Add, Some(b"hi"))],
        &kept,
        &HashMap::new(),
        &mut b,
        &mut Budget::new(&Limits::default()),
    )
    .unwrap();
    assert!(applied.tree.contains_key("trunk/a.txt"));
    assert_eq!(added_paths(&applied.ops), vec!["trunk/a.txt".to_string()]);
    assert!(applied.copies.is_empty());
}

#[test]
fn a_directory_copy_expands_to_per_file_adds_with_stated_copy_records() {
    let mut b = builder();
    // r0: trunk/a and trunk/sub/c exist.
    let r0 = apply_revision(
        &Tree::new(),
        vec![
            file_node("trunk/a.txt", NodeAction::Add, Some(b"a")),
            file_node("trunk/sub/c.txt", NodeAction::Add, Some(b"c")),
        ],
        &HashMap::new(),
        &HashMap::new(),
        &mut b,
        &mut Budget::new(&Limits::default()),
    )
    .unwrap();
    // r0 is referenced by r1's copyfrom, so it is retained (RFC 010 increment 1).
    let mut kept: HashMap<u64, Tree> = HashMap::new();
    kept.insert(0u64, r0.tree.clone());
    // r0's atom (a stand-in id — this test exercises apply_revision directly, without going through
    // IrBuilder::add_atom for each revision).
    let r0_atom = AtomId([1u8; 32]);
    let mut revnum_to_atom: HashMap<u64, AtomId> = HashMap::new();
    revnum_to_atom.insert(0, r0_atom);

    // r1: copy trunk -> branches/x (a branch creation), from r0.
    let r1 = apply_revision(
        &r0.tree,
        vec![dir_copy("branches/x", 0, "trunk")],
        &kept,
        &revnum_to_atom,
        &mut b,
        &mut Budget::new(&Limits::default()),
    )
    .unwrap();

    // every trunk file is now present under branches/x ...
    assert!(r1.tree.contains_key("branches/x/a.txt"));
    assert!(r1.tree.contains_key("branches/x/sub/c.txt"));
    // ... as Add ops ...
    let mut adds = added_paths(&r1.ops);
    adds.sort();
    assert_eq!(adds, vec!["branches/x/a.txt", "branches/x/sub/c.txt"]);
    // ... each with a Stated copy record back to its trunk origin, naming r0's atom.
    assert_eq!(r1.copies.len(), 2);
    assert!(
        r1.copies
            .iter()
            .all(|c| c.status == brygge_ir::EpistemicStatus::Stated && c.from_atom == r0_atom)
    );
    assert!(
        r1.copies
            .iter()
            .any(|c| c.from == "trunk/a.txt" && c.to == "branches/x/a.txt")
    );

    // r2: delete branches/x (whole subtree) -> Delete ops for both files. No copyfrom, so `kept` is
    // unchanged; the base is simply the previous tree.
    revnum_to_atom.insert(1, AtomId([2u8; 32]));
    let r2 = apply_revision(
        &r1.tree,
        vec![NodeRecord {
            path: "branches/x".to_string(),
            kind: None,
            action: NodeAction::Delete,
            copyfrom: None,
            props: None,
            text: None,
            checksums: Checksums::default(),
        }],
        &kept,
        &revnum_to_atom,
        &mut b,
        &mut Budget::new(&Limits::default()),
    )
    .unwrap();
    assert!(!r2.tree.contains_key("branches/x/a.txt"));
    assert!(!r2.tree.contains_key("branches/x/sub/c.txt"));
    let deletes = r2
        .ops
        .iter()
        .filter(|o| matches!(o, PathOp::Delete { .. }))
        .count();
    assert_eq!(deletes, 2);
}

#[test]
fn a_copy_from_an_older_revision_resolves_to_the_correct_from_atom() {
    // Proves the revnum -> AtomId map is consulted by revision number, not by "the previous atom" —
    // a copy sourced from a revision several steps back must still resolve correctly (RFC 011 §5).
    let mut b = builder();
    let r0 = apply_revision(
        &Tree::new(),
        vec![file_node("trunk/a.txt", NodeAction::Add, Some(b"a"))],
        &HashMap::new(),
        &HashMap::new(),
        &mut b,
        &mut Budget::new(&Limits::default()),
    )
    .unwrap();
    let r0_atom = AtomId([9u8; 32]);
    let mut kept: HashMap<u64, Tree> = HashMap::new();
    kept.insert(0u64, r0.tree.clone());
    let mut revnum_to_atom: HashMap<u64, AtomId> = HashMap::new();
    revnum_to_atom.insert(0, r0_atom);

    // r1, r2: unrelated intervening revisions with their own atoms.
    let r1 = apply_revision(
        &r0.tree,
        vec![file_node("unrelated/b.txt", NodeAction::Add, Some(b"b"))],
        &kept,
        &revnum_to_atom,
        &mut b,
        &mut Budget::new(&Limits::default()),
    )
    .unwrap();
    revnum_to_atom.insert(1, AtomId([1u8; 32]));
    let r2 = apply_revision(
        &r1.tree,
        vec![file_node("unrelated/c.txt", NodeAction::Add, Some(b"c"))],
        &kept,
        &revnum_to_atom,
        &mut b,
        &mut Budget::new(&Limits::default()),
    )
    .unwrap();
    revnum_to_atom.insert(2, AtomId([2u8; 32]));

    // r3: copy a single file from r0 — two revisions back.
    let copy_node = NodeRecord {
        path: "trunk/copy-of-a.txt".to_string(),
        kind: Some(NodeKind::File),
        action: NodeAction::Add,
        copyfrom: Some((0, "trunk/a.txt".to_string())),
        props: Some(PropBlock::Full(Vec::new())),
        text: None,
        checksums: Checksums::default(),
    };
    let r3 = apply_revision(
        &r2.tree,
        vec![copy_node],
        &kept,
        &revnum_to_atom,
        &mut b,
        &mut Budget::new(&Limits::default()),
    )
    .unwrap();

    assert_eq!(r3.copies.len(), 1);
    assert_eq!(r3.copies[0].from_atom, r0_atom);
    assert_eq!(r3.copies[0].from, "trunk/a.txt");
    assert_eq!(r3.copies[0].to, "trunk/copy-of-a.txt");
}

/// Review 151 D-1 (a): a directory copy's snapshot expansion pushes a copy record for a file, and the
/// same revision also restates that exact file with its own explicit `replace`-with-copyfrom, from the
/// identical source revision and path (the shape diagnosed on a real repository: codeblocks r5310). The
/// explicit node's `Replace` action voids the directory-expansion's earlier record for that path before
/// `apply_file` pushes its own — so exactly one record survives, not two (which `IrBuilder::add_atom`
/// would otherwise refuse as a non-canonical duplicate).
#[test]
fn a_same_source_replace_under_a_directory_copy_gives_one_copy_record() {
    let mut b = builder();
    let r0 = apply_revision(
        &Tree::new(),
        vec![file_node("trunk/a.txt", NodeAction::Add, Some(b"a"))],
        &HashMap::new(),
        &HashMap::new(),
        &mut b,
        &mut Budget::new(&Limits::default()),
    )
    .unwrap();
    let r0_atom = AtomId([1u8; 32]);
    let mut kept: HashMap<u64, Tree> = HashMap::new();
    kept.insert(0u64, r0.tree.clone());
    let mut revnum_to_atom: HashMap<u64, AtomId> = HashMap::new();
    revnum_to_atom.insert(0, r0_atom);

    // r1: copy trunk -> branches/x (pushes one copy record for a.txt), then the same revision also
    // explicitly replaces branches/x/a.txt with its own copyfrom from the identical source.
    let r1 = apply_revision(
        &r0.tree,
        vec![
            dir_copy("branches/x", 0, "trunk"),
            file_replace_with_copy("branches/x/a.txt", 0, "trunk/a.txt"),
        ],
        &kept,
        &revnum_to_atom,
        &mut b,
        &mut Budget::new(&Limits::default()),
    )
    .unwrap();

    assert_eq!(r1.copies.len(), 1, "{:?}", r1.copies);
    assert_eq!(r1.copies[0].from, "trunk/a.txt");
    assert_eq!(r1.copies[0].to, "branches/x/a.txt");
    assert_eq!(r1.copies[0].from_atom, r0_atom);
}

/// Review 151 D-1 (b): the case a collapse-by-equality rule cannot cover. A directory copy's snapshot
/// expansion pushes a record for a file from source A, and the same revision explicitly replaces that
/// file with a copy from a *different* source B. Only B describes the result — the directory-copy's
/// record for that path must be voided, not kept alongside the new one (which would be two different
/// `from`s for one destination: a false second copy claim, not a duplicate).
#[test]
fn a_different_source_replace_under_a_directory_copy_keeps_only_the_replacement() {
    let mut b = builder();
    let r0 = apply_revision(
        &Tree::new(),
        vec![file_node("trunk/a.txt", NodeAction::Add, Some(b"a"))],
        &HashMap::new(),
        &HashMap::new(),
        &mut b,
        &mut Budget::new(&Limits::default()),
    )
    .unwrap();
    let r0_atom = AtomId([1u8; 32]);
    let mut kept: HashMap<u64, Tree> = HashMap::new();
    kept.insert(0u64, r0.tree.clone());
    let mut revnum_to_atom: HashMap<u64, AtomId> = HashMap::new();
    revnum_to_atom.insert(0, r0_atom);

    let r1 = apply_revision(
        &r0.tree,
        vec![file_node("other/b.txt", NodeAction::Add, Some(b"b"))],
        &kept,
        &revnum_to_atom,
        &mut b,
        &mut Budget::new(&Limits::default()),
    )
    .unwrap();
    let r1_atom = AtomId([2u8; 32]);
    kept.insert(1u64, r1.tree.clone());
    revnum_to_atom.insert(1, r1_atom);

    // r2: copy trunk -> branches/x (pushes a copy record for a.txt from trunk@r0), then the same
    // revision replaces branches/x/a.txt with a copy from other/b.txt@r1 instead.
    let r2 = apply_revision(
        &r1.tree,
        vec![
            dir_copy("branches/x", 0, "trunk"),
            file_replace_with_copy("branches/x/a.txt", 1, "other/b.txt"),
        ],
        &kept,
        &revnum_to_atom,
        &mut b,
        &mut Budget::new(&Limits::default()),
    )
    .unwrap();

    assert_eq!(r2.copies.len(), 1, "{:?}", r2.copies);
    assert_eq!(r2.copies[0].from, "other/b.txt");
    assert_eq!(r2.copies[0].to, "branches/x/a.txt");
    assert_eq!(r2.copies[0].from_atom, r1_atom);
}

// ---- RFC 020 D-2: max_tree_entries (re-expressed), max_copy_expansion, max_tree_nodes -------------

#[test]
fn an_ordinary_file_by_file_growth_past_the_old_entry_ceiling_is_no_longer_refused() {
    // RFC 020 D-2: `max_tree_entries` used to be checked on every ordinary single-file add (0.8.1), as
    // the only thing standing between an adversarial dump and an unbounded *live* tree. A shared tree
    // (D-1) costs nothing extra to hold at any size, so this is no longer checked here — only where a
    // tree is *expanded* into entries (a directory copy, or `--branch-lines`'s own per-file walk). A
    // plain history that grows one file at a time past the old ceiling must now succeed.
    let mut b = builder();
    let limits = Limits {
        max_tree_entries: 10,
        ..Limits::default()
    };
    let over_nodes: Vec<NodeRecord> = (0..11)
        .map(|i| file_node(&format!("trunk/f{i}.txt"), NodeAction::Add, Some(b"x")))
        .collect();
    let applied = apply_revision(
        &Tree::new(),
        over_nodes,
        &HashMap::new(),
        &HashMap::new(),
        &mut b,
        &mut Budget::new(&limits),
    )
    .expect("an ordinary 11-file revision is no longer bounded by max_tree_entries");
    assert!(applied.tree.contains_key("trunk/f10.txt"));
}

#[test]
fn a_directory_copy_that_would_cross_the_entry_ceiling_is_refused() {
    // RFC 020 D-2: `max_tree_entries` is still checked at the one point that actually expands a tree
    // into entries in one step — a directory copy — against the *resulting* live tree size, even when
    // the copy itself is well within `max_copy_expansion`. `budget.live_entries` is only meaningful
    // threaded continuously across every revision of one decode (as `decode_with` does with one
    // `Budget`), so this test shares one `Budget` across both revisions too, rather than starting a
    // fresh one per `apply_revision` call as the other tests here do for their own, unrelated reasons.
    let mut b = builder();
    let limits = Limits {
        max_tree_entries: 10,
        ..Limits::default()
    };
    let mut budget = Budget::new(&limits);
    // r0: trunk holds 8 files (under max_copy_expansion, comfortably, and under the 10-entry ceiling).
    let r0_nodes: Vec<NodeRecord> = (0..8)
        .map(|i| file_node(&format!("trunk/f{i}.txt"), NodeAction::Add, Some(b"x")))
        .collect();
    let r0 = apply_revision(
        &Tree::new(),
        r0_nodes,
        &HashMap::new(),
        &HashMap::new(),
        &mut b,
        &mut budget,
    )
    .unwrap();
    assert_eq!(budget.live_entries, 8);
    let r0_atom = AtomId([1u8; 32]);
    let mut kept: HashMap<u64, Tree> = HashMap::new();
    kept.insert(0, r0.tree.clone());
    let mut revnum_to_atom: HashMap<u64, AtomId> = HashMap::new();
    revnum_to_atom.insert(0, r0_atom);

    // r1: copy the 8-file trunk into branches/x — 16 live entries after the copy, over a ceiling of 10,
    // even though 8 is well under `max_copy_expansion`.
    match apply_revision(
        &r0.tree,
        vec![dir_copy("branches/x", 0, "trunk")],
        &kept,
        &revnum_to_atom,
        &mut b,
        &mut budget,
    ) {
        Ok(_) => panic!("expected a resource-limit refusal, got Ok"),
        Err(crate::Error::ResourceLimit { .. }) => {}
        Err(other) => panic!("expected a resource-limit refusal, got {other:?}"),
    }
}

#[test]
fn a_directory_copy_over_the_expansion_ceiling_is_refused() {
    let mut b = builder();
    // r0: trunk holds 20 files.
    let r0_nodes: Vec<NodeRecord> = (0..20)
        .map(|i| file_node(&format!("trunk/f{i}.txt"), NodeAction::Add, Some(b"x")))
        .collect();
    let r0 = apply_revision(
        &Tree::new(),
        r0_nodes,
        &HashMap::new(),
        &HashMap::new(),
        &mut b,
        &mut Budget::new(&Limits::default()),
    )
    .unwrap();
    let r0_atom = AtomId([1u8; 32]);
    let mut kept: HashMap<u64, Tree> = HashMap::new();
    kept.insert(0, r0.tree.clone());
    let mut revnum_to_atom: HashMap<u64, AtomId> = HashMap::new();
    revnum_to_atom.insert(0, r0_atom);

    // r1: copy the whole 20-file trunk into branches/x, with the copy-expansion ceiling below 20.
    let limits = Limits {
        max_copy_expansion: 10,
        ..Limits::default()
    };
    match apply_revision(
        &r0.tree,
        vec![dir_copy("branches/x", 0, "trunk")],
        &kept,
        &revnum_to_atom,
        &mut b,
        &mut Budget::new(&limits),
    ) {
        Ok(_) => panic!("expected a resource-limit refusal, got Ok"),
        Err(crate::Error::ResourceLimit { .. }) => {}
        Err(other) => panic!("expected a resource-limit refusal, got {other:?}"),
    }
}

#[test]
fn a_dump_that_crosses_the_tree_node_ceiling_is_refused() {
    // RFC 020 D-2: `max_retained_entries` (0.8.1) is gone — retaining a snapshot is now `Rc::clone`,
    // free. `max_tree_nodes` bounds the tree's *cumulative* allocations instead: every `Dir` clone or
    // creation, and every new file entry, across the whole decode. A long chain of edits to files each
    // under their own new directory forces a fresh `Dir` node per edit (no sharing to exploit, by
    // design of the test), so a small per-decode ceiling is crossed with a small, fast dump.
    let mut b = builder();
    let limits = Limits {
        max_tree_nodes: 20,
        ..Limits::default()
    };
    let mut budget = Budget::new(&limits);
    let mut tree = Tree::new();
    let mut kept = HashMap::new();
    let mut revnum_to_atom = HashMap::new();
    for i in 0..50u64 {
        let applied = apply_revision(
            &tree,
            vec![file_node(
                &format!("d{i}/f.txt"),
                NodeAction::Add,
                Some(b"x"),
            )],
            &kept,
            &revnum_to_atom,
            &mut b,
            &mut budget,
        );
        match applied {
            Ok(a) => {
                tree = a.tree;
                revnum_to_atom.insert(i, AtomId([0u8; 32]));
                kept.insert(i, tree.clone());
            }
            Err(crate::Error::ResourceLimit { .. }) => return,
            Err(other) => panic!("expected a resource-limit refusal, got {other:?}"),
        }
    }
    panic!("expected a resource-limit refusal within 50 revisions of a 20-node ceiling");
}

#[test]
fn a_dir_clone_still_shared_with_a_kept_snapshot_is_charged_and_does_not_mutate_it() {
    // RFC 020 D-1's own correctness trap: `Rc::make_mut` on a node still shared with a retained
    // snapshot must copy, never mutate the snapshot in place.
    let mut b = builder();
    let r0 = apply_revision(
        &Tree::new(),
        vec![file_node("trunk/a.txt", NodeAction::Add, Some(b"a"))],
        &HashMap::new(),
        &HashMap::new(),
        &mut b,
        &mut Budget::new(&Limits::default()),
    )
    .unwrap();
    let mut kept: HashMap<u64, Tree> = HashMap::new();
    kept.insert(0, r0.tree.clone());
    let before = r0.tree.clone();

    // r1 edits a different file under `trunk` — `trunk`'s own `Dir` node is shared with `kept[0]` and
    // `before`, so inserting here must clone it, not mutate the snapshot everyone else still holds.
    let r1 = apply_revision(
        &r0.tree,
        vec![file_node("trunk/b.txt", NodeAction::Add, Some(b"b"))],
        &kept,
        &HashMap::new(),
        &mut b,
        &mut Budget::new(&Limits::default()),
    )
    .unwrap();

    assert!(before.contains_key("trunk/a.txt"));
    assert!(
        !before.contains_key("trunk/b.txt"),
        "the kept snapshot must not see r1's new file"
    );
    assert!(kept[&0].contains_key("trunk/a.txt"));
    assert!(
        !kept[&0].contains_key("trunk/b.txt"),
        "kept[0] must not see r1's new file either"
    );
    assert!(r1.tree.contains_key("trunk/a.txt"));
    assert!(r1.tree.contains_key("trunk/b.txt"));
}

// ---- RFC 020 D-1, handoff §0 level 1: a property test against the flat map --------------------------

/// A small deterministic generator (xorshift64*), matching `svndiff::tests`'s own one (no dependency).
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
}

/// Name stems chosen around the module doc's own ordering pitfall: `.` (`0x2E`) and `-` (`0x2D`) both
/// sort before `/` (`0x2F`), so a bare-name comparison of a directory against a sibling file gets the
/// flat map's own order backwards exactly when names like these collide at one level.
const STEMS: &[&str] = &["a", "a.c", "a-b", "b", "b.c", "m", "m.n", "m-n", "z", "z.z"];

fn random_path(rng: &mut Rng) -> String {
    let depth = 1 + rng.below(3) as usize;
    (0..depth)
        .map(|_| STEMS[rng.below(STEMS.len() as u64) as usize])
        .collect::<Vec<_>>()
        .join("/")
}

fn entry(seed: u64) -> FileEntry {
    FileEntry {
        blob: brygge_ir::BlobId([seed as u8; 32]),
        mode: 0o100_644,
        exec: false,
        special: false,
    }
}

/// Random insert/remove sequences, each checked against a flat `BTreeMap<String, FileEntry>` doing the
/// exact same operations: the new tree's `contains_key`/`get` must agree at every step, and
/// `flat_entries()` must equal the flat map's own `iter()` order exactly (the handoff's own standard —
/// not just "same set", but the identical sequence byte order gives, including through name collisions
/// at the `.`/`-`/`/` boundary `STEMS` is chosen to hit).
#[test]
fn random_operations_match_a_flat_map_at_every_step() {
    let mut rng = Rng(0xf00d_cafe_1357_9bdf);
    for case in 0..200 {
        let mut tree = Tree::new();
        let mut flat: std::collections::BTreeMap<String, FileEntry> =
            std::collections::BTreeMap::new();
        let limits = Limits::default();
        let mut budget = Budget::new(&limits);
        for step in 0..200u64 {
            let path = random_path(&mut rng);
            if rng.below(3) == 0 && !flat.is_empty() {
                // A removal: either the exact path, or (if it names a directory prefix in the flat map
                // too) its whole subtree — mirrors `Tree::remove_subtree`'s own two cases.
                tree.remove_subtree(&path, &mut budget).unwrap();
                let prefix = format!("{path}/");
                flat.retain(|k, _| *k != path && !k.starts_with(&prefix));
            } else {
                let e = entry(step);
                tree.insert(&path, e.clone(), &mut budget).unwrap();
                flat.insert(path.clone(), e);
            }
            // Every path either structure has ever touched: `get`/`contains_key` must agree.
            for probe in &[
                "a".to_string(),
                "a.c".to_string(),
                "a/b".to_string(),
                "a.c/b".to_string(),
                path.clone(),
            ] {
                assert_eq!(
                    tree.get(probe).cloned(),
                    flat.get(probe).cloned(),
                    "case {case} step {step}: get({probe:?}) disagrees"
                );
                assert_eq!(
                    tree.contains_key(probe),
                    flat.contains_key(probe),
                    "case {case} step {step}: contains_key({probe:?}) disagrees"
                );
            }
            let got: Vec<(String, FileEntry)> = tree.flat_entries();
            let want: Vec<(String, FileEntry)> =
                flat.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            assert_eq!(
                got, want,
                "case {case} step {step}: iteration order disagrees"
            );
        }
    }
}
