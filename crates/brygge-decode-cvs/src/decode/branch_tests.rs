//! Tests for CVS branch history (RFC 013 D-3, step C-1: branches cut from the main line), on `,v` files written
//! by hand from a small specification. The real-`cvs` fixtures are in `cvs_fixtures.rs`; these run everywhere
//! and cover the shapes a real tool makes hard to reach (a symbol naming a missing revision, a cut across
//! reconstructed changesets, exact dates).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use brygge_ir::Ir;
use brygge_ir::model::{PathOp, RefKind};
use brygge_ir::status::{DerivationKind, EpistemicStatus};

use super::decode;
use crate::{Options, Source};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A temporary CVS repository directory of `,v` files.
struct Repo {
    dir: PathBuf,
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Repo {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("brygge-cvs-br-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn write(&self, rel: &str, bytes: &[u8]) {
        let p = self.dir.join(format!("{rel},v"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }
}

/// One revision of a hand-written `,v`: its number, when, who, what log, and its (one-line) text.
#[derive(Clone)]
struct R {
    num: &'static str,
    date: &'static str,
    log: &'static str,
    text: &'static str,
    dead: bool,
}

fn rev(num: &'static str, date: &'static str, log: &'static str, text: &'static str) -> R {
    R {
        num,
        date,
        log,
        text,
        dead: false,
    }
}

fn dead(num: &'static str, date: &'static str, log: &'static str, text: &'static str) -> R {
    R {
        dead: true,
        ..rev(num, date, log, text)
    }
}

/// A branch cut from trunk revision `point`, numbered `no`, with its revisions in order (`point.no.1`, ...).
struct Br {
    point: &'static str,
    no: u32,
    revs: Vec<R>,
}

fn one_line_delta(from: &str, to: &str) -> String {
    if from == to {
        String::new()
    } else {
        format!("d1 1\na1 1\n{to}\n")
    }
}

/// Write a `,v`: `trunk` in ascending order (`1.1` ... `1.n`), the branches, and the symbols. Every text is one
/// line; a delta is a one-line replace (or empty when the text does not change, as for a dead revision).
fn vfile(trunk: &[R], branches: &[Br], symbols: &[(&str, &str)]) -> Vec<u8> {
    let n = trunk.len();
    let sym = if symbols.is_empty() {
        "symbols;".to_string()
    } else {
        let body: Vec<String> = symbols.iter().map(|(a, b)| format!("{a}:{b}")).collect();
        format!("symbols\n\t{};", body.join("\n\t"))
    };
    let mut s = format!("head\t1.{n};\naccess;\n{sym}\nlocks; strict;\n\n\n");
    let state = |r: &R| if r.dead { "dead" } else { "Exp" };
    let branch_num = |b: &Br, k: usize| format!("{}.{}.{}", b.point, b.no, k);
    for k in (1..=n).rev() {
        let t = &trunk[k - 1];
        assert_eq!(
            t.num,
            format!("1.{k}"),
            "trunk revisions are given in order"
        );
        let starts: Vec<String> = branches
            .iter()
            .filter(|b| b.point == format!("1.{k}") && !b.revs.is_empty())
            .map(|b| branch_num(b, 1))
            .collect();
        let brs = if starts.is_empty() {
            "branches;\n".to_string()
        } else {
            format!("branches\n\t{};\n", starts.join("\n\t"))
        };
        let next = if k > 1 {
            format!("1.{}", k - 1)
        } else {
            String::new()
        };
        s.push_str(&format!(
            "1.{k}\ndate\t{};\tauthor alice;\tstate {};\n{brs}next\t{next};\n\n",
            t.date,
            state(t)
        ));
        for b in branches.iter().filter(|b| b.point == format!("1.{k}")) {
            for (j, r) in b.revs.iter().enumerate() {
                assert_eq!(
                    r.num,
                    branch_num(b, j + 1),
                    "branch revisions are given in order"
                );
                let next = if j + 1 < b.revs.len() {
                    branch_num(b, j + 2)
                } else {
                    String::new()
                };
                s.push_str(&format!(
                    "{}\ndate\t{};\tauthor alice;\tstate {};\nbranches;\nnext\t{next};\n\n",
                    branch_num(b, j + 1),
                    r.date,
                    state(r)
                ));
            }
        }
    }
    s.push_str("\ndesc\n@@\n\n\n");
    s.push_str(&format!(
        "1.{n}\nlog\n@{}@\ntext\n@{}\n@\n",
        trunk[n - 1].log,
        trunk[n - 1].text
    ));
    // The deltas of the branches cut from revision `k`, forward from that revision's text.
    let branch_deltas = |k: usize, s: &mut String| {
        for b in branches.iter().filter(|b| b.point == format!("1.{k}")) {
            let mut from = trunk[k - 1].text;
            for (j, r) in b.revs.iter().enumerate() {
                s.push_str(&format!(
                    "\n{}\nlog\n@{}@\ntext\n@{}@\n",
                    branch_num(b, j + 1),
                    r.log,
                    one_line_delta(from, r.text)
                ));
                from = r.text;
            }
        }
    };
    branch_deltas(n, &mut s);
    for k in (1..n).rev() {
        s.push_str(&format!(
            "\n1.{k}\nlog\n@{}@\ntext\n@{}@\n",
            trunk[k - 1].log,
            one_line_delta(trunk[k].text, trunk[k - 1].text)
        ));
        branch_deltas(k, &mut s);
    }
    s.into_bytes()
}

fn with_refs() -> Options {
    Options {
        reconstruct_refs: true,
        ..Options::default()
    }
}

fn decode_repo(r: &Repo, opts: &Options) -> Ir {
    decode(&Source::LocalRepo(r.dir.clone()), opts).unwrap()
}

/// The tree (path -> text) after atom `idx`, replaying its ancestry from the root.
fn tree_at(ir: &Ir, idx: usize) -> BTreeMap<String, String> {
    let by_id: BTreeMap<_, _> = ir
        .atoms
        .iter()
        .enumerate()
        .map(|(i, a)| (a.id, i))
        .collect();
    let mut chain = vec![idx];
    let mut cur = idx;
    while let Some(p) = ir.atoms[cur].parents.first() {
        cur = by_id[p];
        chain.push(cur);
    }
    let mut tree = BTreeMap::new();
    for &i in chain.iter().rev() {
        for op in &ir.atoms[i].ops {
            match op {
                PathOp::Add { path, blob, .. } | PathOp::Modify { path, blob, .. } => {
                    let text = String::from_utf8(ir.content.get(blob).unwrap().to_vec()).unwrap();
                    tree.insert(path.clone(), text.trim_end().to_string());
                }
                PathOp::Delete { path, .. } => {
                    tree.remove(path);
                }
                other => panic!("unexpected op {other:?}"),
            }
        }
    }
    tree
}

fn atom_index(ir: &Ir, source_prefix: &str) -> Option<usize> {
    ir.atoms
        .iter()
        .position(|a| a.source.atom_id.starts_with(source_prefix.as_bytes()))
}

fn ref_target(ir: &Ir, name: &str) -> usize {
    let r = ir.refs.iter().find(|r| r.name == name).unwrap();
    ir.atoms.iter().position(|a| a.id == r.target).unwrap()
}

fn tree(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(a, b)| ((*a).to_string(), (*b).to_string()))
        .collect()
}

fn params_of(ir: &Ir, idx: usize) -> BTreeMap<String, String> {
    match &ir.atoms[idx].status {
        EpistemicStatus::Derived(d) => d.params.clone(),
        other => panic!("expected a Derived status, got {other:?}"),
    }
}

/// Test 1 (handoff §7): without `--reconstruct-refs` a repository with branches decodes the main line exactly
/// as it always did, and only the drop-reason text is new.
#[test]
fn without_the_flag_the_main_line_is_unchanged_and_only_the_reason_text_is_new() {
    let r = Repo::new();
    r.write(
        "a.c",
        &vfile(
            &[
                rev("1.1", "2024.01.01.00.00.00", "one", "a1"),
                rev("1.2", "2024.01.02.00.00.00", "two", "a2"),
            ],
            &[Br {
                point: "1.1",
                no: 2,
                revs: vec![rev("1.1.2.1", "2024.01.03.00.00.00", "on the branch", "ab")],
            }],
            &[("BR", "1.1.0.2")],
        ),
    );
    let plain = decode_repo(&r, &Options::default());
    assert_eq!(plain.atoms.len(), 2, "the main line only");
    assert!(plain.refs.is_empty());
    let drop = plain
        .loss
        .dropped
        .iter()
        .find(|d| d.what.starts_with("CVS branch revisions not imported"))
        .expect("the branch revision is counted");
    assert_eq!(
        drop.reason,
        "CVS branches are imported only with `--reconstruct-refs`: a branch is identified by its symbol \
         name. Keep the source repository."
    );
    // With the flag, the same main line plus the branch.
    let refs = decode_repo(&r, &with_refs());
    assert_eq!(refs.atoms.len(), 3);
    for a in &plain.atoms {
        assert!(
            refs.atoms.iter().any(|b| b.id == a.id),
            "every main-line atom is unchanged"
        );
    }
}

/// Test 2: a full-tree branch with commits: the exact parent, no branch-point atom, and each branch atom's tree is
/// what `cvs checkout -r BR` gives at that point.
#[test]
fn a_full_tree_branch_has_an_exact_parent_and_no_branch_point_atom() {
    let r = Repo::new();
    // c0: a1.1 and b1.1; c1: a1.2; c2: b1.2. The branch is cut with a at 1.2 and b at 1.1.
    r.write(
        "a.c",
        &vfile(
            &[
                rev("1.1", "2024.01.01.00.00.00", "start", "a1"),
                rev("1.2", "2024.01.02.00.00.00", "edit a", "a2"),
            ],
            &[Br {
                point: "1.2",
                no: 2,
                revs: vec![rev(
                    "1.2.2.1",
                    "2024.02.01.00.00.00",
                    "branch a",
                    "a-branch",
                )],
            }],
            &[("BR", "1.2.0.2")],
        ),
    );
    r.write(
        "b.c",
        &vfile(
            &[
                rev("1.1", "2024.01.01.00.00.05", "start", "b1"),
                rev("1.2", "2024.01.03.00.00.00", "edit b", "b2"),
            ],
            &[Br {
                point: "1.1",
                no: 2,
                revs: vec![rev(
                    "1.1.2.1",
                    "2024.02.02.00.00.00",
                    "branch b",
                    "b-branch",
                )],
            }],
            &[("BR", "1.1.0.2")],
        ),
    );
    let ir = decode_repo(&r, &with_refs());
    // main: c0 (a1.1 + b1.1), c1 (a1.2), c2 (b1.2); branch: two changesets.
    assert_eq!(ir.atoms.len(), 5);
    assert!(
        atom_index(&ir, "branch-point:").is_none(),
        "no branch-point atom"
    );
    assert!(
        ir.flags
            .iter()
            .all(|f| f.what != "CVS branch point spans reconstructed changesets")
    );

    let first = atom_index(&ir, "a.c@1.2.2.1").unwrap();
    let second = atom_index(&ir, "b.c@1.1.2.1").unwrap();
    // The parent is c1 (the changeset of a1.2): the earliest at which both files are at their branch point.
    let c1 = atom_index(&ir, "a.c@1.2").unwrap();
    assert_eq!(ir.atoms[first].parents, vec![ir.atoms[c1].id]);
    let p = params_of(&ir, first);
    assert_eq!(p["branch_point_rule"], "earliest-covering-changeset");
    assert_eq!(p["branch_point"], "exact");
    assert_eq!(p["line"], "BR");
    // Each branch atom's tree is `cvs checkout -r BR` at that point.
    assert_eq!(
        tree_at(&ir, first),
        tree(&[("a.c", "a-branch"), ("b.c", "b1")])
    );
    assert_eq!(
        tree_at(&ir, second),
        tree(&[("a.c", "a-branch"), ("b.c", "b-branch")])
    );
    // The trunk's later change to b never reaches the branch (no merge is inferred).
    assert_eq!(ir.atoms[second].parents, vec![ir.atoms[first].id]);
    // The branch ref is at its last atom.
    assert_eq!(ref_target(&ir, "BR"), second);
    let branch_ref = ir.refs.iter().find(|r| r.name == "BR").unwrap();
    assert_eq!(branch_ref.kind, RefKind::Branch);
    // Every branch changeset is Derived(ReconstructedChangeset), with today's params.
    for i in [first, second] {
        match &ir.atoms[i].status {
            EpistemicStatus::Derived(d) => {
                assert_eq!(d.kind, DerivationKind::ReconstructedChangeset);
                assert_eq!(d.params["confidence_rule"], "span-overlap-v1");
            }
            other => panic!("{other:?}"),
        }
    }
    // Deterministic.
    let again = decode_repo(&r, &with_refs());
    assert_eq!(brygge_ir::to_bytes(&ir), brygge_ir::to_bytes(&again));
}

/// Test 3: a branch of a subdirectory: a branch-point atom deletes the rest, and the tree is only the files on the
/// branch.
#[test]
fn a_subdirectory_branch_gets_a_branch_point_atom_that_deletes_the_rest() {
    let r = Repo::new();
    r.write(
        "top.c",
        &vfile(
            &[rev("1.1", "2024.01.01.00.00.00", "start", "top")],
            &[],
            &[],
        ),
    );
    r.write(
        "sub/x.c",
        &vfile(
            &[rev("1.1", "2024.01.01.00.00.02", "start", "x1")],
            &[Br {
                point: "1.1",
                no: 2,
                revs: vec![rev(
                    "1.1.2.1",
                    "2024.02.01.00.00.00",
                    "branch x",
                    "x-branch",
                )],
            }],
            &[("BR", "1.1.0.2")],
        ),
    );
    r.write(
        "sub/y.c",
        &vfile(
            &[rev("1.1", "2024.01.01.00.00.03", "start", "y1")],
            &[],
            &[("BR", "1.1.0.2")],
        ),
    );
    let ir = decode_repo(&r, &with_refs());
    let bp = atom_index(&ir, "branch-point:BR").expect("a branch-point atom");
    // No author, log or time: it asserts nothing the source did not say.
    assert!(ir.atoms[bp].metadata.author.is_none());
    assert!(ir.atoms[bp].metadata.message.is_none());
    assert!(ir.atoms[bp].metadata.author_time.is_none());
    match &ir.atoms[bp].status {
        EpistemicStatus::Derived(d) => assert_eq!(d.kind, DerivationKind::ReconstructedBranch),
        other => panic!("{other:?}"),
    }
    let p = params_of(&ir, bp);
    assert_eq!(p["branch_point"], "exact");
    // Its ops: delete the file that is not on the branch, each Derived(ReconstructedBranch).
    assert_eq!(ir.atoms[bp].ops.len(), 1);
    match &ir.atoms[bp].ops[0] {
        PathOp::Delete { path, status } => {
            assert_eq!(path, "top.c");
            match status {
                EpistemicStatus::Derived(d) => {
                    assert_eq!(d.kind, DerivationKind::ReconstructedBranch);
                    assert_eq!(d.params["rule"], "branch-point-tree");
                }
                other => panic!("{other:?}"),
            }
        }
        other => panic!("expected a Delete, got {other:?}"),
    }
    assert_eq!(
        tree_at(&ir, bp),
        tree(&[("sub/x.c", "x1"), ("sub/y.c", "y1")])
    );
    let branch_atom = atom_index(&ir, "sub/x.c@1.1.2.1").unwrap();
    assert_eq!(ir.atoms[branch_atom].parents, vec![ir.atoms[bp].id]);
    assert_eq!(
        tree_at(&ir, branch_atom),
        tree(&[("sub/x.c", "x-branch"), ("sub/y.c", "y1")])
    );
    // determinism
    assert_eq!(
        brygge_ir::to_bytes(&ir),
        brygge_ir::to_bytes(&decode_repo(&r, &with_refs()))
    );
}

/// Test 4: a trunk file added after the cut, and not tagged: the parent is before it (earliest covering).
#[test]
fn a_trunk_file_added_after_the_cut_is_not_in_the_parent() {
    let r = Repo::new();
    r.write(
        "a.c",
        &vfile(
            &[rev("1.1", "2024.01.01.00.00.00", "start", "a1")],
            &[Br {
                point: "1.1",
                no: 2,
                revs: vec![rev(
                    "1.1.2.1",
                    "2024.03.01.00.00.00",
                    "branch a",
                    "a-branch",
                )],
            }],
            &[("BR", "1.1.0.2")],
        ),
    );
    r.write(
        "z.c",
        &vfile(
            &[rev("1.1", "2024.02.01.00.00.00", "added later", "z1")],
            &[],
            &[],
        ),
    );
    let ir = decode_repo(&r, &with_refs());
    assert!(atom_index(&ir, "branch-point:").is_none());
    let c0 = atom_index(&ir, "a.c@1.1").unwrap();
    let branch_atom = atom_index(&ir, "a.c@1.1.2.1").unwrap();
    assert_eq!(
        ir.atoms[branch_atom].parents,
        vec![ir.atoms[c0].id],
        "before z.c was added"
    );
    assert_eq!(tree_at(&ir, branch_atom), tree(&[("a.c", "a-branch")]));
}

/// Test 5: a file added on the branch (a dead `1.1` on the trunk) does not constrain the parent, and the branch
/// revision adds it.
#[test]
fn a_file_added_on_the_branch_is_not_in_the_coverage() {
    let r = Repo::new();
    r.write(
        "a.c",
        &vfile(
            &[rev("1.1", "2024.01.01.00.00.00", "start", "a1")],
            &[Br {
                point: "1.1",
                no: 2,
                revs: vec![rev(
                    "1.1.2.1",
                    "2024.03.01.00.00.00",
                    "branch edit",
                    "a-branch",
                )],
            }],
            &[("BR", "1.1.0.2")],
        ),
    );
    r.write(
        "q.c",
        &vfile(
            &[dead(
                "1.1",
                "2024.02.01.00.00.00",
                "file q was initially added on branch BR.",
                "q1",
            )],
            &[Br {
                point: "1.1",
                no: 2,
                revs: vec![rev("1.1.2.1", "2024.03.02.00.00.00", "add q", "q-branch")],
            }],
            &[("BR", "1.1.0.2")],
        ),
    );
    let ir = decode_repo(&r, &with_refs());
    let c0 = atom_index(&ir, "a.c@1.1").unwrap();
    let first = atom_index(&ir, "a.c@1.1.2.1").unwrap();
    assert_eq!(ir.atoms[first].parents, vec![ir.atoms[c0].id]);
    let qa = atom_index(&ir, "q.c@1.1.2.1").unwrap();
    assert!(
        ir.atoms[qa]
            .ops
            .iter()
            .any(|op| matches!(op, PathOp::Add { path, .. } if path == "q.c")),
        "the branch revision adds the file: {:?}",
        ir.atoms[qa].ops
    );
    assert_eq!(
        tree_at(&ir, qa),
        tree(&[("a.c", "a-branch"), ("q.c", "q-branch")])
    );
    assert_eq!(params_of(&ir, first)["branch_point"], "exact");
}

/// Test 6: an approximate branch point (files tagged at different trunk changesets): a `ConventionViolation`, a
/// branch-point atom, `branch_point = approximate`, and the tree exact.
#[test]
fn an_approximate_cut_is_flagged_and_its_tree_is_still_exact() {
    let r = Repo::new();
    // c0: a1.1 + b1.1; c1: a1.2; c2: b1.2. a is tagged at 1.1 (its run is [0, 1)), b at 1.2 (run [2, end)).
    r.write(
        "a.c",
        &vfile(
            &[
                rev("1.1", "2024.01.01.00.00.00", "start", "a1"),
                rev("1.2", "2024.01.02.00.00.00", "edit a", "a2"),
            ],
            &[Br {
                point: "1.1",
                no: 2,
                revs: vec![rev(
                    "1.1.2.1",
                    "2024.03.01.00.00.00",
                    "branch a",
                    "a-branch",
                )],
            }],
            &[("BR", "1.1.0.2")],
        ),
    );
    r.write(
        "b.c",
        &vfile(
            &[
                rev("1.1", "2024.01.01.00.00.05", "start", "b1"),
                rev("1.2", "2024.01.03.00.00.00", "edit b", "b2"),
            ],
            &[],
            &[("BR", "1.2.0.2")],
        ),
    );
    let ir = decode_repo(&r, &with_refs());
    let flag = ir
        .flags
        .iter()
        .find(|f| f.what == "CVS branch point spans reconstructed changesets")
        .expect("the approximate branch is flagged");
    assert_eq!(flag.count, 1);
    assert!(flag.reason.contains("earliest-covering-changeset"));
    let bp = atom_index(&ir, "branch-point:BR").expect("a branch-point atom");
    let p = params_of(&ir, bp);
    assert_eq!(p["branch_point"], "approximate");
    assert_eq!(p["branch_point_rule"], "earliest-covering-changeset");
    // The parent is the earliest changeset with the most files at their branch point: c0 (a1.1 and b1.1; a is
    // at its branch point there). The branch-point atom sets b to its true branch-point content, b1.2.
    let c0 = atom_index(&ir, "a.c@1.1").unwrap();
    assert_eq!(ir.atoms[bp].parents, vec![ir.atoms[c0].id]);
    assert_eq!(tree_at(&ir, bp), tree(&[("a.c", "a1"), ("b.c", "b2")]));
    let branch_atom = atom_index(&ir, "a.c@1.1.2.1").unwrap();
    assert_eq!(
        tree_at(&ir, branch_atom),
        tree(&[("a.c", "a-branch"), ("b.c", "b2")])
    );
    assert_eq!(
        brygge_ir::to_bytes(&ir),
        brygge_ir::to_bytes(&decode_repo(&r, &with_refs()))
    );
}

/// Test 7: a branch tag with no commits: the ref is at the parent (its tree is the parent's, so there is no
/// branch-point atom either).
#[test]
fn a_branch_with_no_commits_is_a_ref_at_its_parent() {
    let r = Repo::new();
    r.write(
        "a.c",
        &vfile(
            &[
                rev("1.1", "2024.01.01.00.00.00", "start", "a1"),
                rev("1.2", "2024.01.02.00.00.00", "edit", "a2"),
            ],
            &[],
            &[("BR", "1.1.0.2")],
        ),
    );
    let ir = decode_repo(&r, &with_refs());
    assert!(atom_index(&ir, "branch-point:").is_none());
    let c0 = atom_index(&ir, "a.c@1.1").unwrap();
    assert_eq!(ref_target(&ir, "BR"), c0);
    assert_eq!(ir.atoms.len(), 2, "no atom for the empty branch");
    assert!(
        !ir.loss
            .dropped
            .iter()
            .any(|d| d.what.starts_with("CVS branch symbols not reconstructed"))
    );
}

/// Test 8: unnamed branch revisions (no symbol) are dropped and counted.
#[test]
fn unnamed_branch_revisions_are_dropped_and_counted() {
    let r = Repo::new();
    r.write(
        "a.c",
        &vfile(
            &[rev("1.1", "2024.01.01.00.00.00", "start", "a1")],
            &[Br {
                point: "1.1",
                no: 2,
                revs: vec![
                    rev("1.1.2.1", "2024.02.01.00.00.00", "b1", "x1"),
                    rev("1.1.2.2", "2024.02.02.00.00.00", "b2", "x2"),
                ],
            }],
            &[],
        ),
    );
    let ir = decode_repo(&r, &with_refs());
    assert_eq!(ir.atoms.len(), 1, "the main line only");
    let drop = ir
        .loss
        .dropped
        .iter()
        .find(|d| {
            d.what
                .starts_with("CVS branch revisions on unnamed branches")
        })
        .expect("counted");
    assert_eq!(
        drop.what,
        "CVS branch revisions on unnamed branches not imported (2)"
    );
    assert!(!drop.reason.contains("planned"));
}

/// Test 9: two commits within the window on the branch with the same (author, log) as a trunk commit stay
/// separate from the trunk; the two branch commits (one file each, same log, close in time) cluster together.
#[test]
fn branch_clustering_is_within_the_branch_only() {
    let r = Repo::new();
    for (path, when_branch) in [
        ("a.c", "2024.03.01.00.00.10"),
        ("b.c", "2024.03.01.00.00.20"),
    ] {
        r.write(
            path,
            &vfile(
                &[
                    rev("1.1", "2024.01.01.00.00.00", "start", "v1"),
                    rev("1.2", "2024.03.01.00.00.00", "fix", "v2"),
                ],
                &[Br {
                    point: "1.1",
                    no: 2,
                    revs: vec![rev("1.1.2.1", when_branch, "fix", "vb")],
                }],
                &[("BR", "1.1.0.2")],
            ),
        );
    }
    let ir = decode_repo(&r, &with_refs());
    // main: c0 (start, both), c1 ("fix", both 1.2s); branch: ONE changeset ("fix", both branch revisions), not
    // merged with the trunk's "fix" although author, log and time window are the same.
    assert_eq!(ir.atoms.len(), 3);
    let branch = atom_index(&ir, "a.c@1.1.2.1").unwrap();
    assert_eq!(
        ir.atoms[branch].source.atom_id,
        b"a.c@1.1.2.1\nb.c@1.1.2.1".to_vec(),
        "one branch changeset with both files"
    );
    let trunk_fix = atom_index(&ir, "a.c@1.2").unwrap();
    assert_ne!(branch, trunk_fix);
    assert_eq!(params_of(&ir, branch)["line"], "BR");
}

/// A tag on branch revisions resolves to the branch changeset that holds them.
#[test]
fn a_tag_on_a_branch_revision_resolves_to_the_branch_changeset() {
    let r = Repo::new();
    r.write(
        "a.c",
        &vfile(
            &[rev("1.1", "2024.01.01.00.00.00", "start", "a1")],
            &[Br {
                point: "1.1",
                no: 2,
                revs: vec![rev("1.1.2.1", "2024.03.01.00.00.00", "branch a", "ab")],
            }],
            &[("BR", "1.1.0.2"), ("REL_ON_BR", "1.1.2.1")],
        ),
    );
    let ir = decode_repo(&r, &with_refs());
    let branch_atom = atom_index(&ir, "a.c@1.1.2.1").unwrap();
    assert_eq!(ref_target(&ir, "REL_ON_BR"), branch_atom);
    assert_eq!(
        ir.refs.iter().find(|r| r.name == "REL_ON_BR").unwrap().kind,
        RefKind::Tag
    );
    assert!(!ir.loss.dropped.iter().any(|d| {
        d.what
            .starts_with("CVS tags on branch revisions not reconstructed")
    }));
}

/// A `cvs import`ed file, still on its vendor branch (the default), then `cvs tag -b BR` with no local edit:
/// the branch point is the vendor revision `1.1.1.1`, so the symbol is `BR:1.1.1.1.0.2` (six components) and
/// the branch's one revision is `1.1.1.1.2.1`. `br` is the branch revision's text.
fn vendor_then_branch(br: &str) -> Vec<u8> {
    format!(
        "head\t1.1;\nbranch\t1.1.1;\naccess;\nsymbols\n\tBR:1.1.1.1.0.2;\nlocks; strict;\n\n\n\
1.1\ndate\t2024.01.01.00.00.00;\tauthor alice;\tstate Exp;\nbranches\n\t1.1.1.1;\nnext\t;\n\n\
1.1.1.1\ndate\t2024.01.01.00.00.00;\tauthor alice;\tstate Exp;\nbranches\n\t1.1.1.1.2.1;\nnext\t;\n\n\
1.1.1.1.2.1\ndate\t2024.02.01.00.00.00;\tauthor alice;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\
1.1\nlog\n@import@\ntext\n@vendor\n@\n\n\
1.1.1.1\nlog\n@Initial revision@\ntext\n@@\n\n\
1.1.1.1.2.1\nlog\n@on the branch@\ntext\n@d1 1\na1 1\n{br}\n@\n"
    )
    .into_bytes()
}

/// Review 036 F-1: a branch cut from a **vendor-branch** revision that is still the default is a main-line
/// branch, however long its symbol. It is imported: the parent is the import changeset, there is no
/// branch-point atom, and the tree has the file at its branch revision (as `cvs checkout -r BR` gives).
#[test]
fn a_branch_cut_from_a_vendor_revision_is_a_main_line_branch() {
    let r = Repo::new();
    r.write("f.c", &vendor_then_branch("br"));
    let ir = decode_repo(&r, &with_refs());

    let tip = ref_target(&ir, "BR");
    assert_eq!(tree_at(&ir, tip), tree(&[("f.c", "br")]));
    assert!(
        atom_index(&ir, "branch-point:").is_none(),
        "the parent's tree is the branch's tree: no branch-point atom"
    );
    let import = atom_index(&ir, "f.c@1.1.1.1").expect("the import changeset");
    assert_eq!(ir.atoms[tip].parents, vec![ir.atoms[import].id]);
    assert_eq!(tree_at(&ir, import), tree(&[("f.c", "vendor")]));
    assert!(
        !ir.loss
            .dropped
            .iter()
            .any(|d| d.what.starts_with("CVS branches whose parent line")),
        "{:?}",
        ir.loss.dropped
    );
    assert!(
        !ir.flags
            .iter()
            .any(|f| f.what == "CVS branch point spans reconstructed changesets"),
        "the parent is exact"
    );
}

/// The mixed case: one file still on its vendor default (a six-component symbol), one locally modified (a
/// four-component symbol). The symbol is a main-line branch in both.
#[test]
fn a_symbol_cut_from_a_vendor_revision_in_one_file_and_a_trunk_revision_in_another_is_one_branch() {
    let r = Repo::new();
    r.write("a.c", &vendor_then_branch("abr"));
    r.write(
        "b.c",
        &vfile(
            &[
                rev("1.1", "2024.01.02.00.00.00", "b start", "b1"),
                rev("1.2", "2024.01.03.00.00.00", "b edit", "b2"),
            ],
            &[Br {
                point: "1.2",
                no: 2,
                revs: vec![rev("1.2.2.1", "2024.02.02.00.00.00", "b branch", "bbr")],
            }],
            &[("BR", "1.2.0.2")],
        ),
    );
    let ir = decode_repo(&r, &with_refs());
    let tip = ref_target(&ir, "BR");
    assert_eq!(tree_at(&ir, tip), tree(&[("a.c", "abr"), ("b.c", "bbr")]));
    assert!(
        !ir.loss
            .dropped
            .iter()
            .any(|d| d.what.starts_with("CVS branches whose parent line")),
        "{:?}",
        ir.loss.dropped
    );
}

/// Once the vendor branch is no longer the default, a revision on it is on no imported line: a symbol cut from
/// it has no parent line to hang from, and is counted, not imported.
#[test]
fn a_branch_cut_from_a_vendor_revision_after_the_vendor_branch_was_cleared_is_counted() {
    let r = Repo::new();
    let cleared = String::from_utf8(vendor_then_branch("br"))
        .unwrap()
        .replace("branch\t1.1.1;\n", "");
    r.write("f.c", cleared.as_bytes());
    let ir = decode_repo(&r, &with_refs());
    assert!(!ir.refs.iter().any(|r| r.name == "BR"));
    let d = ir
        .loss
        .dropped
        .iter()
        .find(|d| {
            d.what
                .starts_with("CVS branches whose parent line is not imported")
        })
        .expect("counted");
    assert_eq!(
        d.what,
        "CVS branches whose parent line is not imported (1 branches, 1 revisions)"
    );
}

/// A file with a branch `BR` (`1.2.0.2`, revisions `1.2.2.1` and `1.2.2.2`) and, when `deep`, a branch cut from
/// the branch revision `1.2.2.1` (`1.2.2.1.0.2`, one revision `1.2.2.1.2.1`). `p` prefixes every text, so two
/// files differ. `symbols` is the body of the symbols table.
fn nested_v(p: &str, symbols: &str, deep: bool) -> Vec<u8> {
    let nest_admin = if deep { "1.2.2.1.2.1" } else { "" };
    let brs = if deep {
        format!("branches\n\t{nest_admin};\n")
    } else {
        "branches;\n".to_string()
    };
    let mut s = format!(
        "head\t1.2;\naccess;\nsymbols\n\t{symbols};\nlocks; strict;\n\n\n\
1.2\ndate\t2024.01.02.00.00.00;\tauthor alice;\tstate Exp;\nbranches\n\t1.2.2.1;\nnext\t1.1;\n\n\
1.1\ndate\t2024.01.01.00.00.00;\tauthor alice;\tstate Exp;\nbranches;\nnext\t;\n\n\
1.2.2.1\ndate\t2024.02.01.00.00.00;\tauthor alice;\tstate Exp;\n{brs}next\t1.2.2.2;\n\n\
1.2.2.2\ndate\t2024.02.03.00.00.00;\tauthor alice;\tstate Exp;\nbranches;\nnext\t;\n\n"
    );
    if deep {
        s.push_str(
            "1.2.2.1.2.1\ndate\t2024.03.01.00.00.00;\tauthor alice;\tstate Exp;\nbranches;\nnext\t;\n\n",
        );
    }
    s.push_str(&format!(
        "\ndesc\n@@\n\n\
1.2\nlog\n@edit@\ntext\n@{p}2\n@\n\n\
1.1\nlog\n@start@\ntext\n@d1 1\na1 1\n{p}1\n@\n\n\
1.2.2.1\nlog\n@br one@\ntext\n@d1 1\na1 1\n{p}b1\n@\n\n\
1.2.2.2\nlog\n@br two@\ntext\n@d1 1\na1 1\n{p}b2\n@\n"
    ));
    if deep {
        s.push_str(&format!(
            "\n1.2.2.1.2.1\nlog\n@nest@\ntext\n@d1 1\na1 1\n{p}n1\n@\n"
        ));
    }
    s.into_bytes()
}

/// Handoff test 10: a branch cut from a branch revision has that branch as its parent line, and hangs from the
/// branch changeset that holds the branch-point revision (not from the branch's tip).
#[test]
fn a_nested_branch_hangs_from_the_branch_changeset_of_its_branch_point() {
    let r = Repo::new();
    r.write(
        "a.c",
        &nested_v("a", "BR:1.2.0.2\n\tNEST:1.2.2.1.0.2", true),
    );
    let ir = decode_repo(&r, &with_refs());

    let br1 = atom_index(&ir, "a.c@1.2.2.1").expect("BR's first changeset");
    let br2 = atom_index(&ir, "a.c@1.2.2.2").expect("BR's second changeset");
    let nest = ref_target(&ir, "NEST");
    assert_eq!(
        ir.atoms[nest].parents,
        vec![ir.atoms[br1].id],
        "parent: the branch point's changeset"
    );
    assert_eq!(tree_at(&ir, nest), tree(&[("a.c", "an1")]));
    assert_eq!(tree_at(&ir, ref_target(&ir, "BR")), tree(&[("a.c", "ab2")]));
    assert_eq!(ir.atoms[br2].parents, vec![ir.atoms[br1].id]);
    assert!(
        atom_index(&ir, "branch-point:NEST").is_none(),
        "the parent's tree is the branch's tree at the cut"
    );
    let params = params_of(&ir, nest);
    assert_eq!(params["line"], "NEST");
    assert_eq!(params["parent_line"], "BR");
    assert_eq!(params["branch_point"], "exact");
    assert!(ir.flags.is_empty(), "{:?}", ir.flags);
    assert!(
        ir.loss
            .dropped
            .iter()
            .all(|d| !d.what.contains("parent line"))
    );
}

/// A branch cut from a branch that has no symbol in its file has no parent line to hang from: dropped and
/// counted, never guessed. The unnamed parent branch's own revisions are dropped as before.
#[test]
fn a_branch_cut_from_an_unnamed_branch_is_dropped_and_counted() {
    let r = Repo::new();
    r.write("a.c", &nested_v("a", "NEST:1.2.2.1.0.2", true));
    let ir = decode_repo(&r, &with_refs());
    assert!(!ir.refs.iter().any(|r| r.name == "NEST"));
    let d = ir
        .loss
        .dropped
        .iter()
        .find(|d| {
            d.what
                .starts_with("CVS branches whose parent line is not imported")
        })
        .expect("counted");
    assert_eq!(
        d.what,
        "CVS branches whose parent line is not imported (1 branches, 1 revisions)"
    );
    assert!(
        ir.loss
            .dropped
            .iter()
            .any(|d| d.what == "CVS branch revisions on unnamed branches not imported (2)"),
        "the unnamed branch's own two revisions: {:?}",
        ir.loss.dropped
    );
}

/// Handoff test 10, the mixed working copy: a symbol whose branch points lie on two lines. The line holding
/// most of them is the parent; the files whose branch point lies elsewhere are on the branch (its tree at the
/// cut has them at their branch-point content), but do not constrain the parent, so it is approximate and a
/// branch-point atom sets their content.
#[test]
fn a_symbol_cut_from_two_lines_takes_the_majority_line_as_parent_and_is_approximate() {
    let r = Repo::new();
    // a.c and b.c: NEST is cut from the branch revision 1.2.2.1 of BR. c.c: NEST is cut from trunk 1.2.
    r.write(
        "a.c",
        &nested_v("a", "BR:1.2.0.2\n\tNEST:1.2.2.1.0.2", true),
    );
    r.write(
        "b.c",
        &nested_v("b", "BR:1.2.0.2\n\tNEST:1.2.2.1.0.2", true),
    );
    r.write("c.c", &nested_v("c", "BR:1.2.0.2\n\tNEST:1.2.0.4", false));
    let ir = decode_repo(&r, &with_refs());

    let nest = ref_target(&ir, "NEST");
    // a.c and b.c's nested revision is the branch's one changeset; c.c has no revision on NEST, so it is at
    // its branch point (1.2, "c2") in the branch's tree.
    assert_eq!(
        tree_at(&ir, nest),
        tree(&[("a.c", "an1"), ("b.c", "bn1"), ("c.c", "c2")])
    );
    let point =
        atom_index(&ir, "branch-point:NEST").expect("c.c's content is set by a branch-point atom");
    let params = params_of(&ir, point);
    assert_eq!(params["branch_point"], "approximate");
    assert_eq!(params["parent_line"], "BR");
    assert!(
        ir.flags
            .iter()
            .any(|f| f.what == "CVS branch point spans reconstructed changesets" && f.count == 1),
        "{:?}",
        ir.flags
    );
}

/// The tie goes to the main line: a symbol cut from the trunk in one file and from a branch revision in
/// another. Another symbol on the second file's own branch is unaffected.
#[test]
fn a_tie_between_lines_goes_to_the_main_line() {
    let r = Repo::new();
    // a.c: BR is a branch of the trunk (1.2.0.2, revision 1.2.2.1).
    r.write(
        "a.c",
        &vfile(
            &[
                rev("1.1", "2024.01.01.00.00.00", "a start", "a1"),
                rev("1.2", "2024.01.02.00.00.00", "a edit", "a2"),
            ],
            &[Br {
                point: "1.2",
                no: 2,
                revs: vec![rev("1.2.2.1", "2024.02.01.00.00.00", "a branch", "abr")],
            }],
            &[("BR", "1.2.0.2")],
        ),
    );
    // b.c: BR is a branch of the branch 1.2.2 (1.2.2.1.0.2, revision 1.2.2.1.2.1); the branch 1.2.2 itself is
    // named OTHER here.
    let b = b"head\t1.2;\naccess;\nsymbols\n\tBR:1.2.2.1.0.2\n\tOTHER:1.2.0.2;\nlocks; strict;\n\n\n\
1.2\ndate\t2024.01.02.00.00.00;\tauthor alice;\tstate Exp;\nbranches\n\t1.2.2.1;\nnext\t1.1;\n\n\
1.1\ndate\t2024.01.01.00.00.00;\tauthor alice;\tstate Exp;\nbranches;\nnext\t;\n\n\
1.2.2.1\ndate\t2024.02.01.00.00.00;\tauthor alice;\tstate Exp;\nbranches\n\t1.2.2.1.2.1;\nnext\t;\n\n\
1.2.2.1.2.1\ndate\t2024.03.01.00.00.00;\tauthor alice;\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\
1.2\nlog\n@b edit@\ntext\n@b2\n@\n\n\
1.1\nlog\n@b start@\ntext\n@d1 1\na1 1\nb1\n@\n\n\
1.2.2.1\nlog\n@on other@\ntext\n@d1 1\na1 1\nbo\n@\n\n\
1.2.2.1.2.1\nlog\n@nested@\ntext\n@d1 1\na1 1\nbn\n@\n";
    r.write("b.c", b);
    let ir = decode_repo(&r, &with_refs());

    // BR: parent line Main (a.c: trunk; b.c: OTHER, a tie), approximate; b.c's tree at the cut is its branch
    // point on OTHER ("bo"), then its nested revision.
    assert_eq!(
        tree_at(&ir, ref_target(&ir, "BR")),
        tree(&[("a.c", "abr"), ("b.c", "bn")])
    );
    let point =
        atom_index(&ir, "branch-point:BR").expect("b.c's content is set by a branch-point atom");
    let params = params_of(&ir, point);
    assert_eq!(params["branch_point"], "approximate");
    assert!(
        !params.contains_key("parent_line"),
        "the parent line is the main line"
    );
    // OTHER is b.c's own branch, cut from the trunk.
    assert_eq!(
        tree_at(&ir, ref_target(&ir, "OTHER")),
        tree(&[("b.c", "bo")])
    );
    assert!(
        ir.loss
            .dropped
            .iter()
            .all(|d| !d.what.contains("parent line"))
    );
}

/// `magic_branch`: the form of the symbol's number decides whether it names a branch; where the branch is
/// cut from is decided by the line of its branch point (`line_of`), tested through the decodes above.
#[test]
fn magic_branch_reads_the_number_not_the_line() {
    use crate::branches::{MagicBranch, magic_branch};
    use crate::rcs::RevNum;
    let rn = |s: &str| RevNum(s.split('.').map(|c| c.parse().unwrap()).collect());
    let m = |b: &str, p: &str| {
        Some(MagicBranch {
            branch_id: rn(b),
            point: rn(p),
        })
    };
    assert_eq!(magic_branch(&rn("1.2.0.4")), m("1.2.4", "1.2"));
    assert_eq!(magic_branch(&rn("1.1.1.1.0.2")), m("1.1.1.1.2", "1.1.1.1"));
    assert_eq!(magic_branch(&rn("1.2.2.1.0.2")), m("1.2.2.1.2", "1.2.2.1"));
    assert_eq!(
        magic_branch(&rn("1.2.4.3.2.1.0.6")),
        m("1.2.4.3.2.1.6", "1.2.4.3.2.1")
    );
    // tags, literal (vendor) branch numbers, and other odd lengths are not magic
    assert_eq!(magic_branch(&rn("1.2")), None);
    assert_eq!(magic_branch(&rn("1.1.1")), None);
    assert_eq!(magic_branch(&rn("1.2.3.4")), None);
    assert_eq!(magic_branch(&rn("1.0.2")), None);
}
