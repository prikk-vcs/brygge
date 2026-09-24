//! CVS branch history against the **real `cvs`** (RFC 013 D-3, handoff §7): repositories built with `cvs init`,
//! `add`, `commit`, `tag -b`, `update -r` and `export`, decoded with `--reconstruct-refs`, and each branch tree
//! compared with what `cvs export -r <branch>` gives. Like the hg and SVN tests, they skip where the tool is
//! absent (it is installed on CI's Linux x86_64 job).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use brygge_ir::Ir;
use brygge_ir::model::PathOp;

use super::decode;
use crate::{Options, Source};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn cvs_available() -> bool {
    Command::new("cvs")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// A real CVS repository with a module `proj` and a working copy of it.
struct Cvs {
    base: PathBuf,
    root: PathBuf,
}

impl Drop for Cvs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

impl Cvs {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!("brygge-cvsreal-{}-{n}", std::process::id()));
        std::fs::create_dir_all(base.join("home")).unwrap();
        let root = base.join("repo");
        let cvs = Self { base, root };
        std::fs::create_dir_all(&cvs.root).unwrap();
        cvs.run(&cvs.base, &["init"]);
        // The module is a directory in the repository: checking it out gives an empty working copy.
        std::fs::create_dir_all(cvs.root.join("proj")).unwrap();
        cvs.run(&cvs.base, &["checkout", "proj"]);
        cvs
    }

    /// A repository whose module `proj` was made by `cvs import` (files still on the vendor branch, which is
    /// the default), and a working copy of it.
    fn imported(files: &[(&str, &str)]) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!("brygge-cvsreal-{}-{n}", std::process::id()));
        std::fs::create_dir_all(base.join("home")).unwrap();
        let root = base.join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let cvs = Self { base, root };
        cvs.run(&cvs.base, &["init"]);
        let src = cvs.base.join("src");
        for (rel, text) in files {
            let p = src.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        cvs.run(&src, &["import", "-m", "import", "proj", "VENDOR", "REL_0"]);
        cvs.run(&cvs.base, &["checkout", "proj"]);
        std::thread::sleep(Duration::from_millis(1100));
        cvs
    }

    fn work(&self) -> PathBuf {
        self.base.join("proj")
    }

    fn run(&self, dir: &Path, args: &[&str]) -> String {
        let out = Command::new("cvs")
            .current_dir(dir)
            .env("HOME", self.base.join("home"))
            .env("CVSEDITOR", "true")
            .env("EDITOR", "true")
            .env("LC_ALL", "C")
            .arg("-q")
            .arg("-d")
            .arg(&self.root)
            .args(args)
            .output()
            .expect("run cvs");
        assert!(
            out.status.success(),
            "cvs {args:?} failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn write(&self, rel: &str, text: &str) {
        let p = self.work().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    /// `cvs add` every path (a new directory first), in the working copy's root.
    fn add(&self, rels: &[&str]) {
        for rel in rels {
            if let Some(parent) = Path::new(rel)
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
            {
                if !self.work().join(parent).join("CVS").exists() {
                    self.run(&self.work(), &["add", parent.to_str().unwrap()]);
                }
            }
            self.run(&self.work(), &["add", rel]);
        }
    }

    /// Commit everything changed, under `dir`, with a **distinct** log, and wait a second so that the next
    /// commit has a later RCS date (the decoder orders and clusters by date).
    fn commit_in(&self, dir: &Path, msg: &str) {
        self.run(dir, &["commit", "-m", msg]);
        std::thread::sleep(Duration::from_millis(1100));
    }

    fn commit(&self, msg: &str) {
        self.commit_in(&self.work(), msg);
    }

    /// `cvs export -r <tag>` of the module into a fresh directory, as path -> text.
    fn export(&self, tag: &str) -> BTreeMap<String, String> {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let name = format!("export-{n}");
        self.run(&self.base, &["export", "-r", tag, "-d", &name, "proj"]);
        let mut out = BTreeMap::new();
        collect(&self.base.join(&name), Path::new(""), &mut out);
        out
    }

    fn decode(&self) -> Ir {
        let opts = Options {
            reconstruct_refs: true,
            ..Options::default()
        };
        // `proj` is the module; its files are the repository's files.
        decode(&Source::LocalRepo(self.root.join("proj")), &opts).unwrap()
    }
}

fn collect(dir: &Path, rel: &Path, out: &mut BTreeMap<String, String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        let rel = rel.join(&name);
        if entry.file_type().unwrap().is_dir() {
            collect(&entry.path(), &rel, out);
        } else {
            out.insert(
                rel.to_str().unwrap().replace('\\', "/"),
                std::fs::read_to_string(entry.path())
                    .unwrap()
                    .trim_end()
                    .to_string(),
            );
        }
    }
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

fn ref_atom(ir: &Ir, name: &str) -> usize {
    let r = ir.refs.iter().find(|r| r.name == name).unwrap();
    ir.atoms.iter().position(|a| a.id == r.target).unwrap()
}

fn has_branch_point_atom(ir: &Ir) -> bool {
    ir.atoms
        .iter()
        .any(|a| a.source.atom_id.starts_with(b"branch-point:"))
}

/// The branch atom that is the parent of `idx`, if any.
fn parent_of(ir: &Ir, idx: usize) -> Option<usize> {
    let p = ir.atoms[idx].parents.first()?;
    ir.atoms.iter().position(|a| a.id == *p)
}

/// Test 2: a full-tree branch with commits: no branch-point atom, and each branch atom's tree is what
/// `cvs export -r BR` gave at that point.
#[test]
fn a_full_tree_branch_matches_cvs_export_at_each_commit() {
    if !cvs_available() {
        eprintln!("skipping: cvs not on PATH");
        return;
    }
    let c = Cvs::new();
    c.write("a.c", "a1\n");
    c.write("b.c", "b1\n");
    c.add(&["a.c", "b.c"]);
    c.commit("start");
    c.write("a.c", "a2\n");
    c.commit("edit a on trunk");
    c.run(&c.work(), &["tag", "-b", "BR"]);
    c.run(&c.work(), &["update", "-r", "BR"]);
    c.write("a.c", "a-branch\n");
    c.commit("branch edit a");
    let after_first = c.export("BR");
    c.write("b.c", "b-branch\n");
    c.commit("branch edit b");
    let after_second = c.export("BR");
    // A trunk change after the branch was cut never reaches the branch (CVS records no merges).
    c.run(&c.work(), &["update", "-A"]);
    c.write("b.c", "b-trunk\n");
    c.commit("edit b on trunk");

    let ir = c.decode();
    assert!(
        !has_branch_point_atom(&ir),
        "a full-tree, exact branch has no branch-point atom"
    );
    let tip = ref_atom(&ir, "BR");
    assert_eq!(tree_at(&ir, tip), after_second);
    let before_tip = parent_of(&ir, tip).unwrap();
    assert_eq!(tree_at(&ir, before_tip), after_first);
    let again = c.decode();
    assert_eq!(
        brygge_ir::to_bytes(&ir),
        brygge_ir::to_bytes(&again),
        "deterministic"
    );
}

/// Test 3: a subdirectory branch: a branch-point atom deletes the rest, and the tree equals
/// `cvs export -r BR`.
#[test]
fn a_subdirectory_branch_matches_cvs_export() {
    if !cvs_available() {
        eprintln!("skipping: cvs not on PATH");
        return;
    }
    let c = Cvs::new();
    c.write("top.c", "top\n");
    c.write("sub/x.c", "x1\n");
    c.write("sub/y.c", "y1\n");
    c.add(&["top.c", "sub/x.c", "sub/y.c"]);
    c.commit("start");
    let sub = c.work().join("sub");
    c.run(&sub, &["tag", "-b", "BR"]);
    c.run(&sub, &["update", "-r", "BR"]);
    c.write("sub/x.c", "x-branch\n");
    c.commit_in(&sub, "branch edit x");
    let exported = c.export("BR");
    assert!(
        !exported.contains_key("top.c"),
        "only the tagged files are on the branch: {exported:?}"
    );

    let ir = c.decode();
    assert!(
        has_branch_point_atom(&ir),
        "the rest of the tree is deleted by a branch-point atom"
    );
    let tip = ref_atom(&ir, "BR");
    assert_eq!(tree_at(&ir, tip), exported);
    let again = c.decode();
    assert_eq!(
        brygge_ir::to_bytes(&ir),
        brygge_ir::to_bytes(&again),
        "deterministic"
    );
}

/// Test 4: a trunk file added after the cut and not tagged: the parent is before it (earliest covering).
#[test]
fn a_trunk_file_added_after_the_cut_is_not_in_the_branch() {
    if !cvs_available() {
        eprintln!("skipping: cvs not on PATH");
        return;
    }
    let c = Cvs::new();
    c.write("a.c", "a1\n");
    c.add(&["a.c"]);
    c.commit("start");
    c.run(&c.work(), &["tag", "-b", "BR"]);
    c.write("z.c", "z1\n");
    c.add(&["z.c"]);
    c.commit("add z on trunk");
    c.run(&c.work(), &["update", "-r", "BR"]);
    c.write("a.c", "a-branch\n");
    c.commit("branch edit a");
    let exported = c.export("BR");

    let ir = c.decode();
    assert!(
        !has_branch_point_atom(&ir),
        "the parent precedes z.c, so the trees agree"
    );
    assert_eq!(tree_at(&ir, ref_atom(&ir, "BR")), exported);
}

/// Test 5: a file added on the branch (a dead `1.1` on the trunk) is added by its branch revision.
#[test]
fn a_file_added_on_the_branch_matches_cvs_export() {
    if !cvs_available() {
        eprintln!("skipping: cvs not on PATH");
        return;
    }
    let c = Cvs::new();
    c.write("a.c", "a1\n");
    c.add(&["a.c"]);
    c.commit("start");
    c.run(&c.work(), &["tag", "-b", "BR"]);
    c.run(&c.work(), &["update", "-r", "BR"]);
    c.write("q.c", "q-on-branch\n");
    c.add(&["q.c"]);
    c.commit("add q on the branch");
    let exported = c.export("BR");
    assert!(exported.contains_key("q.c"));

    let ir = c.decode();
    assert_eq!(tree_at(&ir, ref_atom(&ir, "BR")), exported);
}

/// Test 6: an approximate branch point (files tagged one by one with trunk commits between).
#[test]
fn an_approximate_branch_point_is_flagged_and_its_tree_matches_cvs_export() {
    if !cvs_available() {
        eprintln!("skipping: cvs not on PATH");
        return;
    }
    let c = Cvs::new();
    c.write("a.c", "a1\n");
    c.write("b.c", "b1\n");
    c.add(&["a.c", "b.c"]);
    c.commit("start");
    c.run(&c.work(), &["tag", "-b", "BR", "a.c"]);
    c.write("a.c", "a2\n");
    c.commit("edit a");
    c.write("b.c", "b2\n");
    c.commit("edit b");
    c.run(&c.work(), &["tag", "-b", "BR", "b.c"]);
    c.run(&c.work(), &["update", "-r", "BR"]);
    c.write("a.c", "a-branch\n");
    c.commit("branch edit a");
    let exported = c.export("BR");

    let ir = c.decode();
    assert!(
        ir.flags
            .iter()
            .any(|f| f.what == "CVS branch point spans reconstructed changesets"),
        "{:?}",
        ir.flags
    );
    assert!(has_branch_point_atom(&ir));
    assert_eq!(
        tree_at(&ir, ref_atom(&ir, "BR")),
        exported,
        "the tree is exact though the parent is approximate"
    );
    let again = c.decode();
    assert_eq!(
        brygge_ir::to_bytes(&ir),
        brygge_ir::to_bytes(&again),
        "deterministic"
    );
}

/// Test 7: a branch tag with no commits: the ref is at the parent.
#[test]
fn a_branch_with_no_commits_is_a_ref_at_its_parent() {
    if !cvs_available() {
        eprintln!("skipping: cvs not on PATH");
        return;
    }
    let c = Cvs::new();
    c.write("a.c", "a1\n");
    c.add(&["a.c"]);
    c.commit("start");
    c.write("a.c", "a2\n");
    c.commit("edit");
    c.run(&c.work(), &["tag", "-b", "BR"]);
    let exported = c.export("BR");

    let ir = c.decode();
    assert_eq!(tree_at(&ir, ref_atom(&ir, "BR")), exported);
    assert_eq!(ir.atoms.len(), 2, "no atom of its own");
}

/// Test 8: unnamed branch revisions (the branch symbol deleted) are dropped and counted, as before.
#[test]
fn unnamed_branch_revisions_are_dropped_and_counted() {
    if !cvs_available() {
        eprintln!("skipping: cvs not on PATH");
        return;
    }
    let c = Cvs::new();
    c.write("a.c", "a1\n");
    c.add(&["a.c"]);
    c.commit("start");
    c.run(&c.work(), &["tag", "-b", "BR"]);
    c.run(&c.work(), &["update", "-r", "BR"]);
    c.write("a.c", "a-branch\n");
    c.commit("branch edit a");
    c.run(&c.work(), &["update", "-A"]);
    c.run(&c.work(), &["tag", "-d", "-B", "BR"]);

    let ir = c.decode();
    assert!(!ir.refs.iter().any(|r| r.name == "BR"));
    assert!(
        ir.loss
            .dropped
            .iter()
            .any(|d| d.what == "CVS branch revisions on unnamed branches not imported (1)"),
        "{:?}",
        ir.loss.dropped
    );
}

/// Review 036 F-1: `cvs import`, then `cvs tag -b BR` with no local edit: every file is still on its vendor
/// branch, so the symbol is `BR:1.1.1.1.0.2` (six components). It is a main-line branch: imported, its tree
/// equal to `cvs export -r BR`, with no branch-point atom (the parent's tree is the branch's start).
#[test]
fn a_branch_cut_from_a_cvs_import_matches_cvs_export() {
    if !cvs_available() {
        eprintln!("skipping: cvs not on PATH");
        return;
    }
    let c = Cvs::imported(&[("f.c", "import f\n"), ("sub/g.c", "import g\n")]);
    c.run(&c.work(), &["tag", "-b", "BR"]);
    c.run(&c.work(), &["update", "-r", "BR"]);
    c.write("f.c", "f on branch\n");
    c.commit("branch edit f");
    let exported = c.export("BR");
    assert_eq!(exported.len(), 2, "{exported:?}");

    let ir = c.decode();
    assert!(
        !has_branch_point_atom(&ir),
        "no atom needed at the vendor cut"
    );
    assert_eq!(tree_at(&ir, ref_atom(&ir, "BR")), exported);
    assert!(
        !ir.loss.dropped.iter().any(|d| d
            .what
            .starts_with("CVS branches cut from a branch revision")),
        "{:?}",
        ir.loss.dropped
    );
    let again = c.decode();
    assert_eq!(
        brygge_ir::to_bytes(&ir),
        brygge_ir::to_bytes(&again),
        "deterministic"
    );
}

/// Handoff test 10: a branch cut from a branch revision (`cvs tag -b NEST` from a working copy on `BR`) is a
/// line hanging from `BR`; every tree equals `cvs export -r <branch>`, and `BR` itself is unaffected.
#[test]
fn a_nested_branch_matches_cvs_export() {
    if !cvs_available() {
        eprintln!("skipping: cvs not on PATH");
        return;
    }
    let c = Cvs::new();
    c.write("a.c", "a1\n");
    c.write("b.c", "b1\n");
    c.add(&["a.c", "b.c"]);
    c.commit("start");
    c.run(&c.work(), &["tag", "-b", "BR"]);
    c.run(&c.work(), &["update", "-r", "BR"]);
    c.write("a.c", "a on BR one\n");
    c.write("b.c", "b on BR one\n");
    c.commit("BR one");
    // NEST is cut from the working copy's revisions (both on BR): a branch of a branch.
    c.run(&c.work(), &["tag", "-b", "NEST"]);
    c.write("a.c", "a on BR two\n");
    c.commit("BR two");
    c.run(&c.work(), &["update", "-r", "NEST"]);
    c.write("a.c", "a on NEST\n");
    c.commit("NEST one");
    let nest = c.export("NEST");
    let br = c.export("BR");
    assert_eq!(nest["a.c"], "a on NEST");
    assert_eq!(nest["b.c"], "b on BR one");
    assert_eq!(br["a.c"], "a on BR two");

    let ir = c.decode();
    assert_eq!(tree_at(&ir, ref_atom(&ir, "NEST")), nest);
    assert_eq!(tree_at(&ir, ref_atom(&ir, "BR")), br);
    assert!(
        !ir.loss
            .dropped
            .iter()
            .any(|d| d.what.contains("parent line")),
        "{:?}",
        ir.loss.dropped
    );
    let again = c.decode();
    assert_eq!(
        brygge_ir::to_bytes(&ir),
        brygge_ir::to_bytes(&again),
        "deterministic"
    );
}

/// Handoff test 10, the mixed working copy: `cvs tag -b NEST` from a working copy where one file is on `BR`
/// and another is still on the trunk gives a symbol cut from two lines. The tree still equals `cvs export`.
#[test]
fn a_branch_cut_from_a_mixed_working_copy_matches_cvs_export() {
    if !cvs_available() {
        eprintln!("skipping: cvs not on PATH");
        return;
    }
    let c = Cvs::new();
    c.write("a.c", "a1\n");
    c.write("b.c", "b1\n");
    c.add(&["a.c", "b.c"]);
    c.commit("start");
    c.run(&c.work(), &["tag", "-b", "BR"]);
    c.run(&c.work(), &["update", "-r", "BR"]);
    c.write("a.c", "a on BR\n");
    c.commit("BR one");
    // a.c is at 1.1.2.1 (on BR), b.c at 1.1 (the trunk): NEST is cut from both.
    c.run(&c.work(), &["tag", "-b", "NEST"]);
    c.run(&c.work(), &["update", "-r", "NEST"]);
    c.write("a.c", "a on NEST\n");
    c.commit("NEST one");
    let nest = c.export("NEST");
    assert_eq!(nest["a.c"], "a on NEST");
    assert_eq!(nest["b.c"], "b1");

    let ir = c.decode();
    assert_eq!(tree_at(&ir, ref_atom(&ir, "NEST")), nest);
    let again = c.decode();
    assert_eq!(
        brygge_ir::to_bytes(&ir),
        brygge_ir::to_bytes(&again),
        "deterministic"
    );
}

/// Handoff test 11: a tag on a branch revision resolves to the branch changeset that holds it.
#[test]
fn a_tag_on_a_branch_revision_resolves_to_the_branch_changeset() {
    if !cvs_available() {
        eprintln!("skipping: cvs not on PATH");
        return;
    }
    let c = Cvs::new();
    c.write("a.c", "a1\n");
    c.add(&["a.c"]);
    c.commit("start");
    c.run(&c.work(), &["tag", "-b", "BR"]);
    c.run(&c.work(), &["update", "-r", "BR"]);
    c.write("a.c", "a on BR one\n");
    c.commit("BR one");
    c.run(&c.work(), &["tag", "REL_1"]);
    c.write("a.c", "a on BR two\n");
    c.commit("BR two");
    let released = c.export("REL_1");
    assert_eq!(released["a.c"], "a on BR one");

    let ir = c.decode();
    assert_eq!(tree_at(&ir, ref_atom(&ir, "REL_1")), released);
    assert!(
        !ir.loss.dropped.iter().any(|d| d
            .what
            .starts_with("CVS tags on branch revisions not reconstructed")),
        "{:?}",
        ir.loss.dropped
    );
}
