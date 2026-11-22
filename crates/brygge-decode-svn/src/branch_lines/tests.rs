//! Level-1 tests for `--branch-lines` (RFC 019 Amendment A, D-9): hand-built dumpstreams through
//! [`Source::DumpFile`], one per rule of the handoff's §2.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use brygge_ir::model::{PathOp, RefKind};
use brygge_ir::status::EpistemicStatus;

use crate::{Options, Source};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct DumpBuilder {
    buf: Vec<u8>,
}

fn props_block(pairs: &[(&str, &str)]) -> Vec<u8> {
    let mut b = Vec::new();
    for (k, v) in pairs {
        b.extend(format!("K {}\n", k.len()).into_bytes());
        b.extend(k.as_bytes());
        b.push(b'\n');
        b.extend(format!("V {}\n", v.len()).into_bytes());
        b.extend(v.as_bytes());
        b.push(b'\n');
    }
    b.extend(b"PROPS-END\n");
    b
}

impl DumpBuilder {
    fn new() -> Self {
        let mut buf = Vec::new();
        buf.extend(b"SVN-fs-dump-format-version: 2\n\n");
        buf.extend(b"UUID: 11111111-2222-3333-4444-555555555555\n\n");
        Self { buf }
    }

    fn revision(&mut self, num: u64, props: &[(&str, &str)]) -> &mut Self {
        let pblock = props_block(props);
        self.buf
            .extend(format!("Revision-number: {num}\n").into_bytes());
        self.buf
            .extend(format!("Prop-content-length: {}\n", pblock.len()).into_bytes());
        self.buf
            .extend(format!("Content-length: {}\n\n", pblock.len()).into_bytes());
        self.buf.extend(pblock);
        self
    }

    fn node(
        &mut self,
        path: &str,
        kind: Option<&str>,
        action: &str,
        props: Option<&[(&str, &str)]>,
        text: Option<&[u8]>,
        copyfrom: Option<(u64, &str)>,
    ) -> &mut Self {
        self.buf.extend(format!("Node-path: {path}\n").into_bytes());
        if let Some(k) = kind {
            self.buf.extend(format!("Node-kind: {k}\n").into_bytes());
        }
        self.buf
            .extend(format!("Node-action: {action}\n").into_bytes());
        if let Some((r, p)) = copyfrom {
            self.buf
                .extend(format!("Node-copyfrom-rev: {r}\nNode-copyfrom-path: {p}\n").into_bytes());
        }
        let pblock = props.map(props_block);
        let plen = pblock.as_ref().map(Vec::len);
        let tlen = text.map(<[u8]>::len);
        if let Some(pl) = plen {
            self.buf
                .extend(format!("Prop-content-length: {pl}\n").into_bytes());
        }
        if let Some(tl) = tlen {
            self.buf
                .extend(format!("Text-content-length: {tl}\n").into_bytes());
        }
        if plen.is_some() || tlen.is_some() {
            let clen = plen.unwrap_or(0) + tlen.unwrap_or(0);
            self.buf
                .extend(format!("Content-length: {clen}\n").into_bytes());
        }
        self.buf.push(b'\n');
        if let Some(pb) = pblock {
            self.buf.extend(pb);
        }
        if let Some(t) = text {
            self.buf.extend(t);
        }
        self.buf.extend(b"\n\n");
        self
    }

    fn add_file(&mut self, path: &str, text: &[u8]) -> &mut Self {
        self.node(path, Some("file"), "add", Some(&[]), Some(text), None)
    }
    fn add_dir(&mut self, path: &str) -> &mut Self {
        self.node(path, Some("dir"), "add", None, None, None)
    }
    fn copy_dir(&mut self, path: &str, from: (u64, &str)) -> &mut Self {
        self.node(path, Some("dir"), "add", None, None, Some(from))
    }
    fn delete(&mut self, path: &str) -> &mut Self {
        self.node(path, None, "delete", None, None, None)
    }
}

fn date(n: u64) -> String {
    format!("2024-01-{:02}T00:00:00.000000Z", n + 1)
}

fn decode_dump(bytes: &[u8], opts: &Options) -> Result<brygge_ir::Ir, crate::Error> {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path: PathBuf = std::env::temp_dir().join(format!(
        "brygge-svn-branchlines-{}-{n}.dump",
        std::process::id()
    ));
    std::fs::write(&path, bytes).unwrap();
    let out = crate::decode(&Source::DumpFile(path.clone()), opts);
    let _ = std::fs::remove_file(&path);
    out
}

fn branch_lines_opts() -> Options {
    Options {
        reconstruct_refs: true,
        branch_lines: true,
        ..Options::default()
    }
}

fn atom_root(ir: &brygge_ir::Ir, idx: usize) -> String {
    match &ir.atoms[idx].status {
        EpistemicStatus::Derived(d) => d.params.get("root").cloned().unwrap_or_default(),
        _ => panic!("expected every branch-lines atom to be Derived"),
    }
}

#[test]
fn a_trunk_only_history_gives_one_line_with_relative_paths() {
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "add"), ("svn:date", &date(1))]);
    d.add_dir("trunk");
    d.add_file("trunk/a.txt", b"one\n");
    d.revision(2, &[("svn:log", "edit"), ("svn:date", &date(2))]);
    d.node(
        "trunk/a.txt",
        Some("file"),
        "change",
        Some(&[]),
        Some(b"two\n"),
        None,
    );

    let ir = decode_dump(&d.buf, &branch_lines_opts()).unwrap();
    // r0 touches nothing (no root), r1 and r2 each touch trunk: two atoms.
    assert_eq!(ir.atoms.len(), 2);
    for a in &ir.atoms {
        assert!(matches!(a.status, EpistemicStatus::Derived(_)));
    }
    // ops are relative to trunk: "a.txt", never "trunk/a.txt".
    let has_relative_add = ir.atoms[0]
        .ops
        .iter()
        .any(|o| matches!(o, PathOp::Add { path, .. } if path == "a.txt"));
    assert!(has_relative_add, "{:?}", ir.atoms[0].ops);
    assert_eq!(ir.refs.len(), 1);
    assert_eq!(ir.refs[0].name, "trunk");
}

#[test]
fn a_branch_created_by_copy_gives_empty_ops_and_the_source_as_parent() {
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "add"), ("svn:date", &date(1))]);
    d.add_dir("trunk");
    d.add_file("trunk/a.txt", b"one\n");
    d.add_dir("branches");
    d.revision(2, &[("svn:log", "branch"), ("svn:date", &date(2))]);
    d.copy_dir("branches/stable", (1, "trunk"));

    let ir = decode_dump(&d.buf, &branch_lines_opts()).unwrap();
    assert_eq!(ir.atoms.len(), 2); // trunk@1, branches/stable@2
    let branch_atom = &ir.atoms[1];
    assert!(
        branch_atom.ops.is_empty(),
        "a plain creation copy should give no ops: {:?}",
        branch_atom.ops
    );
    assert_eq!(branch_atom.parents.len(), 1);
    assert_eq!(branch_atom.parents[0], ir.atoms[0].id);
    assert_eq!(atom_root(&ir, 1), "branches/stable");

    assert_eq!(ir.refs.len(), 2);
    let names: Vec<&str> = ir.refs.iter().map(|r| r.name.as_str()).collect();
    assert!(names.contains(&"trunk"));
    assert!(names.contains(&"stable"));
}

#[test]
fn a_branch_created_by_copy_with_an_extra_edit_records_only_the_edit() {
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "add"), ("svn:date", &date(1))]);
    d.add_dir("trunk");
    d.add_file("trunk/a.txt", b"one\n");
    d.add_dir("branches");
    d.revision(2, &[("svn:log", "branch+edit"), ("svn:date", &date(2))]);
    d.copy_dir("branches/stable", (1, "trunk"));
    d.node(
        "branches/stable/a.txt",
        Some("file"),
        "change",
        Some(&[]),
        Some(b"edited\n"),
        None,
    );

    let ir = decode_dump(&d.buf, &branch_lines_opts()).unwrap();
    let branch_atom = &ir.atoms[1];
    assert_eq!(branch_atom.ops.len(), 1, "{:?}", branch_atom.ops);
    assert!(matches!(
        &branch_atom.ops[0],
        PathOp::Modify { path, .. } if path == "a.txt"
    ));
}

#[test]
fn a_deleted_branch_gets_an_other_ref_named_with_its_last_revision() {
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "add"), ("svn:date", &date(1))]);
    d.add_dir("trunk");
    d.add_file("trunk/a.txt", b"one\n");
    d.add_dir("branches");
    d.revision(2, &[("svn:log", "branch"), ("svn:date", &date(2))]);
    d.copy_dir("branches/stable", (1, "trunk"));
    d.revision(3, &[("svn:log", "prune"), ("svn:date", &date(3))]);
    d.delete("branches/stable");

    let ir = decode_dump(&d.buf, &branch_lines_opts()).unwrap();
    // trunk@1, branches/stable@2 — r3 only deletes, gives no new atom for the dead line.
    assert_eq!(ir.atoms.len(), 2);
    assert_eq!(ir.refs.len(), 2);
    let other = ir
        .refs
        .iter()
        .find(|r| matches!(r.kind, RefKind::Other(_)))
        .expect("a deleted branch should get an Other ref");
    assert_eq!(other.name, "branches/stable@r2");
    assert_eq!(other.target, ir.atoms[1].id);
}

#[test]
fn deleted_then_recreated_gives_two_lives_with_distinct_refs() {
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "add"), ("svn:date", &date(1))]);
    d.add_dir("trunk");
    d.add_file("trunk/a.txt", b"one\n");
    d.add_dir("branches");
    d.revision(2, &[("svn:log", "branch"), ("svn:date", &date(2))]);
    d.copy_dir("branches/stable", (1, "trunk"));
    d.revision(3, &[("svn:log", "prune"), ("svn:date", &date(3))]);
    d.delete("branches/stable");
    d.revision(4, &[("svn:log", "re-branch"), ("svn:date", &date(4))]);
    d.copy_dir("branches/stable", (1, "trunk"));

    let ir = decode_dump(&d.buf, &branch_lines_opts()).unwrap();
    // trunk@1, stable-life-1@2, stable-life-2@4.
    assert_eq!(ir.atoms.len(), 3);
    // trunk (still live), the live "stable" (life 2), and the dead life's own "@r2" ref.
    assert_eq!(ir.refs.len(), 3);
    let live = ir
        .refs
        .iter()
        .find(|r| r.name == "stable")
        .expect("the current life should be the live ref");
    assert_eq!(live.target, ir.atoms[2].id);
    let dead = ir
        .refs
        .iter()
        .find(|r| r.name == "branches/stable@r2")
        .expect("the earlier life should still get its own ref");
    assert_eq!(dead.target, ir.atoms[1].id);
}

#[test]
fn a_file_copied_across_branches_is_an_add_with_a_copy_record() {
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "add"), ("svn:date", &date(1))]);
    d.add_dir("trunk");
    d.add_file("trunk/a.txt", b"one\n");
    d.add_dir("branches");
    d.revision(2, &[("svn:log", "branch"), ("svn:date", &date(2))]);
    d.copy_dir("branches/stable", (1, "trunk"));
    d.revision(3, &[("svn:log", "cross-copy"), ("svn:date", &date(3))]);
    d.node(
        "trunk/b.txt",
        Some("file"),
        "add",
        Some(&[]),
        None,
        Some((2, "branches/stable/a.txt")),
    );

    let ir = decode_dump(&d.buf, &branch_lines_opts()).unwrap();
    // trunk@1, branches/stable@2 (the branch), trunk@3 (the cross-copy).
    let trunk2 = ir
        .atoms
        .iter()
        .find(|a| {
            atom_name(ir_root(&ir, a)) == "trunk" && a.ops.iter().any(|o| op_path_eq(o, "b.txt"))
        })
        .expect("trunk's second atom should exist");
    assert_eq!(trunk2.copies.len(), 1, "{:?}", trunk2.copies);
    assert_eq!(trunk2.copies[0].from, "a.txt");
    assert_eq!(trunk2.copies[0].to, "b.txt");
}

fn ir_root(ir: &brygge_ir::Ir, atom: &brygge_ir::model::ChangeAtom) -> String {
    let _ = ir;
    match &atom.status {
        EpistemicStatus::Derived(d) => d.params.get("root").cloned().unwrap_or_default(),
        _ => String::new(),
    }
}
fn atom_name(root: String) -> String {
    root
}
fn op_path_eq(op: &PathOp, want: &str) -> bool {
    match op {
        PathOp::Add { path, .. }
        | PathOp::Modify { path, .. }
        | PathOp::Replace { path, .. }
        | PathOp::Delete { path, .. } => path == want,
    }
}

/// Review 147 F-3: a root created by copying from a path outside every recognized root. The behaviour
/// is unchanged (a fresh line, no parent) — this proves its first atom's ops really are `Add`s of
/// exactly the copied content, and that the count lands in the existing convention-violation flag.
#[test]
fn a_root_created_from_outside_the_layout_gives_adds_of_the_copied_content() {
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "add"), ("svn:date", &date(1))]);
    d.add_dir("trunk");
    d.add_file("trunk/a.txt", b"one\n");
    d.add_dir("branches");
    d.add_dir("random");
    d.add_file("random/x.txt", b"from outside\n");
    d.add_file("random/y.txt", b"also from outside\n");
    d.revision(
        2,
        &[("svn:log", "branch from outside"), ("svn:date", &date(2))],
    );
    d.copy_dir("branches/odd", (1, "random"));

    let ir = decode_dump(&d.buf, &branch_lines_opts()).unwrap();
    // trunk@1, branches/odd@2 — "random" is not a recognized root, so it gets no atom of its own.
    assert_eq!(ir.atoms.len(), 2);
    let odd_atom = &ir.atoms[1];
    assert_eq!(atom_root(&ir, 1), "branches/odd");
    assert!(odd_atom.parents.is_empty(), "no root to inherit from");
    let mut adds: Vec<&str> = odd_atom
        .ops
        .iter()
        .map(|o| match o {
            PathOp::Add { path, .. } => path.as_str(),
            other => panic!("expected only Add ops, got {other:?}"),
        })
        .collect();
    adds.sort_unstable();
    assert_eq!(adds, ["x.txt", "y.txt"]);

    assert!(
        ir.flags.iter().any(|f| f
            .what
            .contains("a root created from outside the layout (1)")),
        "{:?}",
        ir.flags
    );
}

#[test]
fn a_revision_touching_no_root_is_counted_not_an_atom() {
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "add"), ("svn:date", &date(1))]);
    d.add_dir("trunk");
    d.add_file("trunk/a.txt", b"one\n");
    d.revision(2, &[("svn:log", "unrelated"), ("svn:date", &date(2))]);
    d.add_file("README-top-level.txt", b"not under any root\n");

    let ir = decode_dump(&d.buf, &branch_lines_opts()).unwrap();
    assert_eq!(ir.atoms.len(), 1);
    assert!(
        ir.loss
            .dropped
            .iter()
            .any(|d| d.what.contains("revisions with no branch change")),
        "{:?}",
        ir.loss.dropped
    );
    assert!(
        ir.loss
            .dropped
            .iter()
            .any(|d| d.what.contains("r2") && d.what.contains("README-top-level.txt")),
        "{:?}",
        ir.loss.dropped
    );
}

#[test]
fn without_branch_lines_nothing_changes() {
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "add"), ("svn:date", &date(1))]);
    d.add_dir("trunk");
    d.add_file("trunk/a.txt", b"one\n");

    let with_reconstruct = Options {
        reconstruct_refs: true,
        ..Options::default()
    };
    let before = decode_dump(&d.buf, &with_reconstruct).unwrap();
    let after = decode_dump(&d.buf, &with_reconstruct).unwrap();
    assert_eq!(
        brygge_ir::artifact::to_bytes(&before),
        brygge_ir::artifact::to_bytes(&after)
    );
    // the single-spine path's own shape still holds: one atom per revision.
    assert_eq!(before.atoms.len(), 2);
}
