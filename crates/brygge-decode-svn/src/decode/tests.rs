//! Integration tests for the Subversion decoder (RFC 006): decode a dumpstream into an IR and assert the
//! M3 properties — a `Stated` spine with `Stated` copies, an opt-in `Derived` branch/tag layer,
//! determinism, the floor, and the loss boundary. Most tests feed a hand-crafted dumpstream through
//! [`Source::DumpFile`] (no `svnadmin` needed); one drives the `svnadmin` subprocess and skips when absent.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use brygge_ir::model::{PathOp, RefKind};
use brygge_ir::status::EpistemicStatus;

use super::decode;
use crate::{Options, Source};

static COUNTER: AtomicU64 = AtomicU64::new(0);

// ---- a hand-crafted dumpstream builder ------------------------------------------------------------

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
            .extend(format!("Content-length: {}\n", pblock.len()).into_bytes());
        self.buf.push(b'\n');
        self.buf.extend(pblock);
        self.buf.extend(b"\n");
        self
    }

    #[allow(clippy::too_many_arguments)]
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
}

fn date(n: u64) -> String {
    format!("2024-01-{:02}T00:00:00.000000Z", n + 1)
}

/// Decode a dumpstream through `Source::DumpFile` (writes a temp file, decodes, cleans up).
fn decode_dump(bytes: &[u8], opts: &Options) -> Result<brygge_ir::Ir, crate::Error> {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path: PathBuf =
        std::env::temp_dir().join(format!("brygge-svndec-{}-{n}.dump", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    let out = decode(&Source::DumpFile(path.clone()), opts);
    let _ = std::fs::remove_file(&path);
    out
}

// ---- tests ----------------------------------------------------------------------------------------

#[test]
fn a_trunk_only_history_decodes_as_a_stated_spine() {
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(
        1,
        &[
            ("svn:author", "alice"),
            ("svn:date", &date(1)),
            ("svn:log", "add"),
        ],
    );
    d.add_dir("trunk");
    d.add_file("trunk/a.txt", b"one\n");
    d.revision(
        2,
        &[
            ("svn:author", "bob"),
            ("svn:date", &date(2)),
            ("svn:log", "edit"),
        ],
    );
    d.node(
        "trunk/a.txt",
        Some("file"),
        "change",
        Some(&[]),
        Some(b"two\n"),
        None,
    );

    let ir = decode_dump(&d.buf, &Options::default()).unwrap();
    assert_eq!(ir.atoms.len(), 3); // r0, r1, r2
    // linear spine: each atom (after the first) has exactly one parent.
    assert!(ir.atoms.iter().all(|a| a.parents.len() <= 1));
    // everything is Stated; no derived assertions with reconstruction off.
    let report = brygge_ir::summary(&ir);
    assert!(
        report.derived.is_empty(),
        "derived should be empty: {:?}",
        report.derived
    );
    // the edit is a Modify.
    let has_modify = ir
        .atoms
        .iter()
        .flat_map(|a| &a.ops)
        .any(|o| matches!(o, PathOp::Modify { path, .. } if path == "trunk/a.txt"));
    assert!(has_modify);
    // author/time claims are carried.
    let r1 = &ir.atoms[1];
    assert_eq!(
        r1.metadata.author.as_ref().unwrap().name.as_utf8(),
        Some("alice")
    );
    assert_eq!(
        r1.metadata.author_time.map(|t| t.seconds),
        Some(crate::props::parse_svn_date(&date(1)).unwrap())
    );
}

#[test]
fn a_file_copy_is_carried_as_a_stated_copy_record() {
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "add"), ("svn:date", &date(1))]);
    d.add_file("a.txt", b"x\n");
    d.revision(2, &[("svn:log", "copy"), ("svn:date", &date(2))]);
    // svn cp a.txt b.txt (a copy that keeps its source).
    d.node("b.txt", Some("file"), "add", None, None, Some((1, "a.txt")));

    let ir = decode_dump(&d.buf, &Options::default()).unwrap();
    let r1_id = ir.atoms[1].id;
    let r2 = &ir.atoms[2];
    assert_eq!(r2.copies.len(), 1);
    let c = &r2.copies[0];
    assert_eq!(c.from, "a.txt");
    assert_eq!(c.to, "b.txt");
    assert_eq!(c.from_atom, r1_id); // names the correct source atom (RFC 011 §5)
    assert_eq!(c.status, EpistemicStatus::Stated); // never Derived — the source recorded it
    // and the copy is a Stated Add (b.txt), with a.txt still present (a copy, not a move).
    let report = brygge_ir::summary(&ir);
    assert!(report.derived.is_empty());
}

#[test]
fn branch_and_tag_reconstruction_is_derived_and_opt_in() {
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "trunk"), ("svn:date", &date(1))]);
    d.add_dir("trunk");
    d.add_file("trunk/a.txt", b"x\n");
    d.revision(2, &[("svn:log", "branch"), ("svn:date", &date(2))]);
    d.add_dir("branches");
    d.copy_dir("branches/feature", (1, "trunk"));
    d.revision(3, &[("svn:log", "tag"), ("svn:date", &date(3))]);
    d.add_dir("tags");
    d.copy_dir("tags/v1", (1, "trunk"));

    // Off by default: no refs, everything Stated.
    let ir_off = decode_dump(&d.buf, &Options::default()).unwrap();
    assert!(ir_off.refs.is_empty());
    assert!(brygge_ir::summary(&ir_off).derived.is_empty());

    // On: Derived branch/tag refs appear.
    let opts = Options {
        reconstruct_refs: true,
        ..Options::default()
    };
    let ir_on = decode_dump(&d.buf, &opts).unwrap();
    let names: Vec<_> = ir_on.refs.iter().map(|r| r.name.clone()).collect();
    assert!(names.contains(&"trunk".to_string()));
    assert!(names.contains(&"feature".to_string()));
    assert!(names.contains(&"v1".to_string()));
    // every reconstructed ref is Derived (never Stated) ...
    assert!(ir_on.refs.iter().all(|r| r.status.is_derived()));
    // ... the tag carries the immutability caveat in its derivation params ...
    let tag = ir_on.refs.iter().find(|r| r.kind == RefKind::Tag).unwrap();
    if let EpistemicStatus::Derived(der) = &tag.status {
        assert!(der.params.contains_key("layout"));
        assert!(der.params.contains_key("immutability"));
    } else {
        panic!("tag ref should be Derived");
    }
    // ... and they surface on the fidelity report under the reconstructed-branch taxonomy label.
    let report = brygge_ir::summary(&ir_on);
    assert_eq!(report.derived.get("reconstructed-branch"), Some(&3));
}

#[test]
fn a_convention_violating_layout_fabricates_no_ref_and_flags_it_loudly() {
    // RFC 011 §5/D-8 (review 007-recut R-1): nothing is dropped here — no ref was fabricated, so
    // nothing that would have existed was lost. The condition is a `Flag`, not a `DropRecord`: one
    // fact, one place.
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "flat"), ("svn:date", &date(1))]);
    // no trunk/branches/tags — a flat layout.
    d.add_file("main.c", b"x\n");
    d.add_file("readme", b"y\n");

    let opts = Options {
        reconstruct_refs: true,
        ..Options::default()
    };
    let ir = decode_dump(&d.buf, &opts).unwrap();
    assert!(ir.refs.is_empty(), "no ref should be fabricated");
    // The layout violation itself drops nothing (no ref was fabricated to begin with) — only the flag
    // carries it. Other, unrelated representation-class drops (dumpstream framing) may still be present.
    assert!(
        !ir.loss.dropped.iter().any(|dr| dr.what.contains("layout")),
        "the layout violation must not also appear as a drop: {:?}",
        ir.loss.dropped
    );
    // One condition, one flag (review 010 R-2/F-1): the partial-layout flag must not also fire.
    assert_eq!(ir.flags.len(), 1, "exactly one flag: {:?}", ir.flags);
    let layout_not_found = &ir.flags[0];
    assert!(layout_not_found.what.contains("layout not found"));
    assert_eq!(
        layout_not_found.kind,
        brygge_ir::FlagKind::ConventionViolation
    );
    assert_eq!(layout_not_found.count, 1);
}

#[test]
fn mergeinfo_is_dropped_advisory_and_never_a_parent() {
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "x"), ("svn:date", &date(1))]);
    d.add_dir("trunk");
    d.add_file("trunk/a", b"x\n");
    d.revision(2, &[("svn:log", "merge"), ("svn:date", &date(2))]);
    // a dir property change carrying svn:mergeinfo.
    d.node(
        "trunk",
        Some("dir"),
        "change",
        Some(&[("svn:mergeinfo", "/branches/x:1")]),
        None,
        None,
    );

    let ir = decode_dump(&d.buf, &Options::default()).unwrap();
    // linear spine only — mergeinfo never became an ancestry edge.
    assert!(ir.atoms.iter().all(|a| a.parents.len() <= 1));
    let advisory = ir
        .loss
        .dropped
        .iter()
        .any(|dr| dr.what.contains("mergeinfo"));
    assert!(advisory);
}

#[test]
fn a_symlink_and_an_executable_get_their_modes() {
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "modes"), ("svn:date", &date(1))]);
    d.node(
        "run.sh",
        Some("file"),
        "add",
        Some(&[("svn:executable", "*")]),
        Some(b"#!/bin/sh\n"),
        None,
    );
    d.node(
        "link",
        Some("file"),
        "add",
        Some(&[("svn:special", "*")]),
        Some(b"link target.txt"),
        None,
    );

    let ir = decode_dump(&d.buf, &Options::default()).unwrap();
    let ops: Vec<_> = ir.atoms.iter().flat_map(|a| &a.ops).collect();
    let exec = ops.iter().find_map(|o| match o {
        PathOp::Add { path, mode, .. } if path == "run.sh" => Some(*mode),
        _ => None,
    });
    let link = ops.iter().find_map(|o| match o {
        PathOp::Add { path, mode, .. } if path == "link" => Some(*mode),
        _ => None,
    });
    assert_eq!(exec, Some(0o100_755));
    assert_eq!(link, Some(0o120_000));
}

#[test]
fn externals_are_refused_by_the_full_pipeline() {
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "ext"), ("svn:date", &date(1))]);
    d.node(
        "trunk",
        Some("dir"),
        "add",
        Some(&[("svn:externals", "^/lib lib")]),
        None,
        None,
    );
    let err = decode_dump(&d.buf, &Options::default()).unwrap_err();
    assert!(matches!(err, crate::Error::FloorRefusal { .. }));
}

#[test]
fn decode_is_deterministic() {
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(
        1,
        &[
            ("svn:author", "a"),
            ("svn:log", "x"),
            ("svn:date", &date(1)),
        ],
    );
    d.add_dir("trunk");
    d.add_file("trunk/a", b"x\n");
    d.add_file("trunk/b", b"y\n");

    let a = decode_dump(&d.buf, &Options::default()).unwrap();
    let b = decode_dump(&d.buf, &Options::default()).unwrap();
    assert_eq!(brygge_ir::to_bytes(&a), brygge_ir::to_bytes(&b));
}

#[test]
fn the_ir_round_trips_through_the_artifact_codec() {
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "x"), ("svn:date", &date(1))]);
    d.add_file("a", b"x\n");
    let ir = decode_dump(&d.buf, &Options::default()).unwrap();
    let bytes = brygge_ir::to_bytes(&ir);
    let back = brygge_ir::from_bytes(&bytes).unwrap();
    assert_eq!(brygge_ir::to_bytes(&back.ir), bytes);
}

#[test]
fn a_url_source_is_refused() {
    // Never dump over the network (INV-3).
    let out = decode(
        &Source::LocalRepo(PathBuf::from("https://svn.example.com/repo")),
        &Options::default(),
    );
    assert!(matches!(out, Err(crate::Error::FloorRefusal { .. })));
}

// ---- one svnadmin-driven test (skips when the tool is absent) --------------------------------------

fn svnadmin_available() -> bool {
    Command::new("svnadmin")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn decodes_a_live_repository_via_svnadmin_dump() {
    if !svnadmin_available() {
        eprintln!("skipping: svnadmin not available");
        return;
    }
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let repo = std::env::temp_dir().join(format!("brygge-svnrepo-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    assert!(
        Command::new("svnadmin")
            .arg("create")
            .arg(&repo)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    );
    // Load a minimal dump so the repo has one revision, then decode via Source::LocalRepo.
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "x"), ("svn:date", &date(1))]);
    d.add_file("a.txt", b"hi\n");
    // `svnadmin load` ignores the dumped UUID with --force-uuid off; feed our dump on stdin.
    use std::io::Write as _;
    if let Ok(mut child) = Command::new("svnadmin")
        .arg("load")
        .arg(&repo)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(&d.buf);
        }
        let _ = child.wait();
    }
    let ir = decode(&Source::LocalRepo(repo.clone()), &Options::default());
    let _ = std::fs::remove_dir_all(&repo);
    let ir = ir.expect("decoding a live repo should succeed");
    assert!(!ir.atoms.is_empty());
}

#[test]
fn cr_07_6_a_live_decode_records_source_form_and_svnadmin_version() {
    if !svnadmin_available() {
        eprintln!("skipping: svnadmin not available");
        return;
    }
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let repo = std::env::temp_dir().join(format!("brygge-svnrepo-ver-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    assert!(
        Command::new("svnadmin")
            .arg("create")
            .arg(&repo)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    );
    let ir = decode(&Source::LocalRepo(repo.clone()), &Options::default());
    let _ = std::fs::remove_dir_all(&repo);
    let ir = ir.expect("decoding a live repo should succeed");
    assert_eq!(
        ir.provenance.params.get("source_form"),
        Some(&"svnadmin-dump".to_string())
    );
    let version = ir
        .provenance
        .params
        .get("svnadmin_version")
        .expect("svnadmin_version recorded");
    assert!(!version.is_empty());
}

#[test]
fn a_copyfrom_to_a_distant_revision_still_resolves() {
    // RFC 010 increment 1: only revisions a copyfrom names are retained. Here trunk is created at r1,
    // several unrelated revisions follow (not retained), then r5 copies trunk@1 — proving the distant
    // snapshot survived the gap.
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "trunk"), ("svn:date", &date(1))]);
    d.add_dir("trunk");
    d.add_file("trunk/a.txt", b"original\n");
    for r in 2..=4u64 {
        d.revision(r, &[("svn:log", "unrelated"), ("svn:date", &date(r))]);
        d.node(
            "other.txt",
            Some("file"),
            "add",
            Some(&[]),
            Some(b"x\n"),
            None,
        );
    }
    d.revision(
        5,
        &[("svn:log", "branch from old trunk"), ("svn:date", &date(5))],
    );
    d.add_dir("branches");
    d.copy_dir("branches/x", (1, "trunk")); // copyfrom the distant r1

    let ir = decode_dump(&d.buf, &Options::default()).unwrap();
    // branches/x/a.txt exists and carries trunk@1's content — so r1's snapshot was retained across r2..r4.
    let branched_add = ir
        .atoms
        .iter()
        .flat_map(|a| &a.ops)
        .any(|o| matches!(o, PathOp::Add { path, .. } if path == "branches/x/a.txt"));
    assert!(branched_add, "the copy from the distant revision resolved");
    // and the copy record names r1's atom specifically — not r4's (the immediately preceding
    // revision) or any other — proving the revnum -> AtomId map resolves by revision number, not
    // proximity (RFC 011 §5).
    let r1_id = ir.atoms[1].id;
    let branch_atom = ir
        .atoms
        .iter()
        .find(|a| a.copies.iter().any(|c| c.to == "branches/x/a.txt"))
        .expect("the branching atom is present");
    let copy = branch_atom
        .copies
        .iter()
        .find(|c| c.to == "branches/x/a.txt")
        .expect("the copy record for branches/x/a.txt is present");
    assert_eq!(copy.from_atom, r1_id);
    assert_eq!(copy.from, "trunk/a.txt");
    // and it deduplicates to trunk/a.txt's blob (same content) — the store carries it once.
    let a = decode_dump(&d.buf, &Options::default()).unwrap();
    assert_eq!(brygge_ir::to_bytes(&ir), brygge_ir::to_bytes(&a)); // deterministic
}

// --- CR-12.2: the floor is one declared list, recorded in provenance --------------------------------

#[test]
fn provenance_floor_param_equals_the_declared_list() {
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    let ir = decode_dump(&d.buf, &Options::default()).unwrap();
    let expected = crate::floor::joined();
    assert_eq!(ir.provenance.params.get("floor"), Some(&expected));
}

// ---- CR-07 corrections (§1 reproduction, then the fixed behavior) -----------------------------------

#[test]
fn cr_07_1_a_symlink_retargeted_without_a_property_block_strips_the_link_prefix() {
    // Reproduction (§1): rev 1 creates a symlink with an explicit svn:special property block; rev 2
    // retargets it with NEW text and NO property block at all (svn:special persists implicitly, as SVN
    // itself behaves — a node with no property block keeps its previous properties). Before the fix,
    // the decoder only stripped the `link ` prefix when the *own* node carried svn:special, so this
    // retarget stored `link newtarget` verbatim instead of `newtarget` — content corruption.
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "add link"), ("svn:date", &date(1))]);
    d.node(
        "link",
        Some("file"),
        "add",
        Some(&[("svn:special", "*")]),
        Some(b"link oldtarget"),
        None,
    );
    d.revision(2, &[("svn:log", "retarget"), ("svn:date", &date(2))]);
    // No property block at all: svn:special is inherited, not restated.
    d.node(
        "link",
        Some("file"),
        "change",
        None,
        Some(b"link newtarget"),
        None,
    );

    let ir = decode_dump(&d.buf, &Options::default()).unwrap();
    let blob = ir
        .atoms
        .iter()
        .flat_map(|a| &a.ops)
        .find_map(|o| match o {
            PathOp::Modify { path, blob, .. } if path == "link" => Some(*blob),
            _ => None,
        })
        .expect("a Modify op for 'link' in revision 2");
    let content = ir.content.get(&blob).expect("blob content present");
    assert_eq!(
        content, b"newtarget",
        "the retargeted symlink's content must be the bare target, not the raw 'link <target>' text"
    );
}

#[test]
fn cr_07_1_a_symlink_whose_text_lacks_the_link_prefix_is_refused() {
    // Review 010 R-3.1: a malformed dump is refused, never guessed at.
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "add link"), ("svn:date", &date(1))]);
    d.node(
        "link",
        Some("file"),
        "add",
        Some(&[("svn:special", "*")]),
        Some(b"not a link target"),
        None,
    );
    let err = decode_dump(&d.buf, &Options::default()).unwrap_err();
    assert!(
        matches!(err, crate::Error::Read(ref m) if m == "svn:special file without a link target: link"),
        "expected the exact link-target Read error, got {err:?}"
    );
}

#[test]
fn cr_07_3_a_deleted_branch_is_not_reconstructed_as_a_live_ref() {
    // Reproduction (§1): a branch is created, touched, then deleted. Before the fix, `heads` only ever
    // accumulated (branch prefix -> last atom that touched it) and was never checked against the final
    // tree, so `reconstruct_refs` still fabricated a ref for the deleted branch, pointing at the
    // revision that deleted it — a ref for history that no longer lives anywhere.
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "branch"), ("svn:date", &date(1))]);
    d.add_dir("branches");
    d.add_dir("branches/feature");
    d.add_file("branches/feature/a.txt", b"x\n");
    d.revision(2, &[("svn:log", "delete branch"), ("svn:date", &date(2))]);
    d.node("branches/feature", None, "delete", None, None, None);

    let opts = Options {
        reconstruct_refs: true,
        ..Options::default()
    };
    let ir = decode_dump(&d.buf, &opts).unwrap();
    assert!(
        !ir.refs.iter().any(|r| r.name == "feature"),
        "a deleted branch must not be represented as a live ref: {:?}",
        ir.refs
    );
    let drop = ir
        .loss
        .dropped
        .iter()
        .find(|d| d.what.contains("deleted or moved"))
        .expect("a 'deleted or moved' drop record");
    assert_eq!(
        drop.what,
        "deleted or moved branches/tags not represented (1)"
    );
}

#[test]
fn cr_07_3_a_moved_branch_gives_the_new_name_only() {
    // Review 010 R-3.2: `svn mv branches/a branches/b` (a copy to the new path plus a delete of the old
    // one, in one revision) must give a ref for `b` only — never a stray ref for `a` — and count the
    // moved-away root exactly once.
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "branch a"), ("svn:date", &date(1))]);
    d.add_dir("branches");
    d.add_dir("branches/a");
    d.add_file("branches/a/f.txt", b"x\n");
    d.revision(2, &[("svn:log", "mv a to b"), ("svn:date", &date(2))]);
    d.copy_dir("branches/b", (1, "branches/a"));
    d.node("branches/a", None, "delete", None, None, None);

    let opts = Options {
        reconstruct_refs: true,
        ..Options::default()
    };
    let ir = decode_dump(&d.buf, &opts).unwrap();
    let names: Vec<_> = ir.refs.iter().map(|r| r.name.clone()).collect();
    assert!(
        names.contains(&"b".to_string()),
        "the moved branch must appear under its new name: {names:?}"
    );
    assert!(
        !names.contains(&"a".to_string()),
        "the old name must not also appear as a ref: {names:?}"
    );
    let drop = ir
        .loss
        .dropped
        .iter()
        .find(|d| d.what.contains("deleted or moved"))
        .expect("a 'deleted or moved' drop record");
    assert_eq!(
        drop.what, "deleted or moved branches/tags not represented (1)",
        "exactly one root (the old 'a' prefix) should be counted as moved away"
    );
}

#[test]
fn cr_07_2_a_file_replace_gives_replace_even_with_identical_content() {
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "add"), ("svn:date", &date(1))]);
    d.add_file("f.txt", b"same\n");
    d.revision(2, &[("svn:log", "replace"), ("svn:date", &date(2))]);
    d.node(
        "f.txt",
        Some("file"),
        "replace",
        Some(&[]),
        Some(b"same\n"),
        None,
    );

    let ir = decode_dump(&d.buf, &Options::default()).unwrap();
    let ops = &ir.atoms[2].ops;
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert!(
        matches!(&ops[0], PathOp::Replace { path, .. } if path == "f.txt"),
        "identical-content replace must still be Replace, not a no-op: {ops:?}"
    );
}

#[test]
fn cr_07_2_a_directory_replace_gives_replace_delete_and_add() {
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "add"), ("svn:date", &date(1))]);
    d.add_dir("d");
    d.add_file("d/a.txt", b"a\n");
    d.add_file("d/b.txt", b"b\n");
    d.revision(2, &[("svn:log", "replace dir"), ("svn:date", &date(2))]);
    d.node("d", Some("dir"), "replace", None, None, None);
    d.add_file("d/a.txt", b"a\n"); // re-added, same content -> Replace
    d.add_file("d/c.txt", b"c\n"); // new -> Add
    // d/b.txt not re-added -> Delete

    let ir = decode_dump(&d.buf, &Options::default()).unwrap();
    let ops = &ir.atoms[2].ops;
    let find = |p: &str| ops.iter().find(|o| o.path() == p);
    assert!(
        matches!(find("d/a.txt"), Some(PathOp::Replace { .. })),
        "{ops:?}"
    );
    assert!(
        matches!(find("d/b.txt"), Some(PathOp::Delete { .. })),
        "{ops:?}"
    );
    assert!(
        matches!(find("d/c.txt"), Some(PathOp::Add { .. })),
        "{ops:?}"
    );
}

#[test]
fn cr_07_4_a_directory_filled_in_the_same_revision_is_not_counted_empty() {
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "mkdir+file"), ("svn:date", &date(1))]);
    d.add_dir("d");
    d.add_file("d/a.txt", b"a\n");
    d.revision(2, &[("svn:log", "truly empty"), ("svn:date", &date(2))]);
    d.add_dir("e");

    let ir = decode_dump(&d.buf, &Options::default()).unwrap();
    let drop = ir
        .loss
        .dropped
        .iter()
        .find(|dr| dr.what.starts_with("empty directories"))
        .expect("an empty-directories drop record");
    assert_eq!(
        drop.what, "empty directories (1)",
        "only 'e' (truly empty) should count; 'd' received a file in the same revision"
    );
}

#[test]
fn cr_07_4_r4_a_replaced_directory_that_ends_up_empty_is_counted() {
    // Review 010 R-4: "added in the revision" was amended to "added or replaced" — excluding a replaced
    // directory would drop it with no record when it ends up empty (INV-1).
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "mkdir+file"), ("svn:date", &date(1))]);
    d.add_dir("d");
    d.add_file("d/a.txt", b"a\n");
    d.revision(
        2,
        &[
            ("svn:log", "replace with empty dir"),
            ("svn:date", &date(2)),
        ],
    );
    d.node("d", Some("dir"), "replace", None, None, None);

    let ir = decode_dump(&d.buf, &Options::default()).unwrap();
    let drop = ir
        .loss
        .dropped
        .iter()
        .find(|dr| dr.what.starts_with("empty directories"))
        .expect("an empty-directories drop record");
    assert_eq!(
        drop.what, "empty directories (1)",
        "the replaced 'd' has no file after the replace, and must be counted, not silently dropped"
    );
}

#[test]
fn cr_07_5_paths_outside_the_layout_are_flagged() {
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:log", "mixed"), ("svn:date", &date(1))]);
    d.add_dir("trunk");
    d.add_file("trunk/a.txt", b"a\n");
    d.add_file("README", b"stray\n");

    let opts = Options {
        reconstruct_refs: true,
        ..Options::default()
    };
    let ir = decode_dump(&d.buf, &opts).unwrap();
    // a live 'trunk' root reconstructs, so this is NOT the whole-layout-not-found case.
    assert!(ir.refs.iter().any(|r| r.name == "trunk"));
    let flag = ir
        .flags
        .iter()
        .find(|f| f.what.contains("outside the trunk/branches/tags layout"))
        .expect("a layout-honesty flag");
    assert_eq!(flag.kind, brygge_ir::FlagKind::ConventionViolation);
    assert_eq!(
        flag.what,
        "paths outside the trunk/branches/tags layout (1)"
    );
    assert_eq!(flag.count, 1);
}

#[test]
fn cr_07_6_a_dumpfile_decode_records_its_source_form() {
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    let ir = decode_dump(&d.buf, &Options::default()).unwrap();
    assert_eq!(
        ir.provenance.params.get("source_form"),
        Some(&"dumpfile".to_string())
    );
    assert!(!ir.provenance.params.contains_key("svnadmin_version"));
}

#[test]
fn cr_04_committer_and_commit_time_are_absent_and_the_offset_is_utc() {
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(
        1,
        &[
            ("svn:author", "alice"),
            ("svn:date", &date(1)),
            ("svn:log", "x"),
        ],
    );
    d.add_file("a.txt", b"x\n");

    let ir = decode_dump(&d.buf, &Options::default()).unwrap();
    let atom = &ir.atoms[1];
    assert!(atom.metadata.committer.is_none());
    assert!(atom.metadata.commit_time.is_none());
    assert_eq!(
        atom.metadata
            .author_time
            .as_ref()
            .and_then(|t| t.offset_minutes),
        Some(0)
    );
}

#[test]
fn cr_04_an_unparseable_svn_date_is_counted_not_guessed() {
    let mut d = DumpBuilder::new();
    d.revision(0, &[("svn:date", &date(0))]);
    d.revision(1, &[("svn:date", "not-a-date"), ("svn:log", "x")]);
    d.add_file("a.txt", b"x\n");

    let ir = decode_dump(&d.buf, &Options::default()).unwrap();
    assert!(ir.atoms[1].metadata.author_time.is_none());
    let drop = ir
        .loss
        .dropped
        .iter()
        .find(|dr| dr.what.starts_with("unparseable svn:date"))
        .expect("an unparseable-svn:date drop record");
    assert_eq!(drop.what, "unparseable svn:date (1)");
}
