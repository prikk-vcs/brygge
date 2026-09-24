//! Delta dumps (RFC 013 D-2): `svnadmin dump --deltas` and `svnrdump dump` (svndiff version 0 and property
//! deltas), the checksums, the missing base, `RR-svn-special-toggle`, and the ceilings.
//!
//! Two kinds of fixture. The **real** ones are built with `svnmucc`, dumped by `svnadmin` (fulltext and
//! `--deltas`) and `svnrdump`, and skip where those tools are absent (CI's Linux x86_64 job has them). The
//! **hand-written** ones feed a crafted dumpstream (svndiff windows written by hand) and run everywhere; they
//! cover the shapes a real tool makes hard to reach (a wrong checksum, a base that is missing, a ceiling).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use brygge_ir::Ir;
use brygge_ir::model::PathOp;

use super::{decode, decode_with};
use crate::checksum::{md5_hex, sha1_hex};
use crate::source::Limits;
use crate::tree::FileEntry;
use crate::{Error, Options, Source};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn tool_available(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// `svnadmin`, `svnrdump` and `svnmucc`, `svn` and `svnlook`: all come with `subversion`.
fn svn_tools() -> bool {
    ["svnadmin", "svnrdump", "svnmucc", "svn", "svnlook"]
        .iter()
        .all(|t| tool_available(t))
}

// ---- a real repository, driven with svnmucc ------------------------------------------------------------

struct Repo {
    base: PathBuf,
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn run(cmd: &mut Command) -> Vec<u8> {
    let out = cmd.output().expect("spawn");
    assert!(
        out.status.success(),
        "{cmd:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

impl Repo {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!("brygge-svndelta-{}-{n}", std::process::id()));
        std::fs::create_dir_all(base.join("w")).unwrap();
        let repo = Self { base };
        run(Command::new("svnadmin").arg("create").arg(repo.path()));
        repo
    }

    fn path(&self) -> PathBuf {
        self.base.join("repo")
    }

    fn url(&self) -> String {
        format!("file://{}", self.path().display())
    }

    /// A local file with these bytes, for `svnmucc put`.
    fn file(&self, name: &str, data: &[u8]) -> String {
        let p = self.base.join("w").join(name);
        std::fs::write(&p, data).unwrap();
        p.display().to_string()
    }

    fn mucc(&self, msg: &str, ops: &[&str]) {
        run(Command::new("svnmucc")
            .args(["-U", &self.url(), "-m", msg])
            .args(ops));
    }

    fn youngest(&self) -> u64 {
        String::from_utf8(run(Command::new("svnlook")
            .arg("youngest")
            .arg(self.path())))
        .unwrap()
        .trim()
        .parse()
        .unwrap()
    }

    fn dump(&self, form: Form) -> Vec<u8> {
        match form {
            Form::Full => run(Command::new("svnadmin")
                .args(["dump", "-q"])
                .arg(self.path())),
            Form::Deltas => run(Command::new("svnadmin")
                .args(["dump", "-q", "--deltas"])
                .arg(self.path())),
            Form::IncrementalFrom(r) => run(Command::new("svnadmin")
                .args([
                    "dump",
                    "-q",
                    "--deltas",
                    "--incremental",
                    "-r",
                    &format!("{r}:HEAD"),
                ])
                .arg(self.path())),
            Form::Rdump => run(Command::new("svnrdump")
                .args(["dump", "-q"])
                .arg(self.url())),
        }
    }
}

#[derive(Clone, Copy)]
enum Form {
    Full,
    Deltas,
    IncrementalFrom(u64),
    Rdump,
}

/// The lines of the "big" file, ~300 KiB (more than the 100 KiB window Subversion splits at).
fn big_text() -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for i in 0..8000 {
        let _ = writeln!(
            out,
            "line {i} of the big file {}",
            (i * 7919 + 13) % 1_000_003
        );
    }
    out
}

/// The repository of handoff test 1: text edits, a binary file and an empty file, a file of more than 100 KiB
/// changed in its middle, copies with and without edits, a replace with a copy, property add/change/delete on
/// files and directories, a symlink retargeted, an executable, and a file with both `svn:special` and
/// `svn:executable` from which one is then deleted. Returns the repository.
fn rich_repo() -> Repo {
    let r = Repo::new();
    let big = big_text();
    let binary: Vec<u8> = (0..=255u8).cycle().take(5120).chain([0, 255, 0]).collect();
    let (a, e, b, bg, sh) = (
        r.file("a", b"one\ntwo\nthree\n"),
        r.file("e", b""),
        r.file("b", &binary),
        r.file("big", big.as_bytes()),
        r.file("sh", b"#!/bin/sh\necho hi\n"),
    );
    r.mucc(
        "r1",
        &[
            "mkdir",
            "trunk",
            "mkdir",
            "trunk/d",
            "mkdir",
            "branches",
            "put",
            &a,
            "trunk/a.txt",
            "put",
            &e,
            "trunk/empty.txt",
            "put",
            &b,
            "trunk/bin.dat",
            "put",
            &bg,
            "trunk/big.txt",
            "put",
            &sh,
            "trunk/run.sh",
            "propset",
            "svn:executable",
            "*",
            "trunk/run.sh",
            "propset",
            "color",
            "red",
            "trunk/d",
        ],
    );
    // Change the middle of the big file: later windows' source views slide against the base.
    let mut lines: Vec<&str> = big.split('\n').collect();
    lines[4000] = "the middle line, edited";
    let big2 = lines.join("\n");
    let mut binary2 = binary.clone();
    binary2.splice(100..100, b"changed".iter().copied());
    let (a2, big2f, b2) = (
        r.file("a2", b"one\nTWO\nthree\nfour\n"),
        r.file("big2", big2.as_bytes()),
        r.file("b2", &binary2),
    );
    r.mucc(
        "r2 edits",
        &[
            "put",
            &a2,
            "trunk/a.txt",
            "put",
            &big2f,
            "trunk/big.txt",
            "put",
            &b2,
            "trunk/bin.dat",
        ],
    );
    let big3 = r.file("big3", big2.replace("line 10 ", "LINE 10 ").as_bytes());
    r.mucc(
        "r3 copies, one edited",
        &[
            "cp",
            "1",
            "trunk/a.txt",
            "trunk/copy-a.txt",
            "cp",
            "2",
            "trunk/big.txt",
            "trunk/big-edited-copy.txt",
            "put",
            &big3,
            "trunk/big-edited-copy.txt",
        ],
    );
    r.mucc(
        "r4 replace with a copy",
        &[
            "rm",
            "trunk/copy-a.txt",
            "cp",
            "3",
            "trunk/a.txt",
            "trunk/copy-a.txt",
        ],
    );
    r.mucc(
        "r5 properties",
        &[
            "propset",
            "custom",
            "v1",
            "trunk/a.txt",
            "propset",
            "color",
            "blue",
            "trunk/d",
            "propset",
            "svn:eol-style",
            "native",
            "trunk/big.txt",
        ],
    );
    r.mucc(
        "r6",
        &[
            "propset",
            "custom",
            "v2",
            "trunk/a.txt",
            "propdel",
            "color",
            "trunk/d",
        ],
    );
    let (ln, ln2, f) = (
        r.file("ln", b"link trunk/a.txt"),
        r.file("ln2", b"link trunk/empty.txt"),
        r.file("f", b"link somewhere"),
    );
    r.mucc(
        "r7 a symlink",
        &[
            "put",
            &ln,
            "trunk/ln",
            "propset",
            "svn:special",
            "*",
            "trunk/ln",
        ],
    );
    r.mucc("r8 retargeted", &["put", &ln2, "trunk/ln"]);
    r.mucc(
        "r9 also executable",
        &["propset", "svn:executable", "*", "trunk/ln"],
    );
    r.mucc(
        "r10 no longer special, still executable",
        &[
            "propdel",
            "svn:special",
            "trunk/ln",
            "propdel",
            "svn:executable",
            "trunk/run.sh",
        ],
    );
    r.mucc(
        "r11 a file whose text is a link",
        &["put", &f, "trunk/f.txt"],
    );
    r.mucc(
        "r12 becomes special, no text",
        &["propset", "svn:special", "*", "trunk/f.txt"],
    );
    r.mucc("r13 a branch", &["cp", "12", "trunk", "branches/b1"]);
    let a3 = r.file("a3", b"branch one\n");
    r.mucc(
        "r14 in the branch",
        &[
            "put",
            &a3,
            "branches/b1/a.txt",
            "propset",
            "svn:mergeinfo",
            "/trunk:1-13",
            "branches/b1",
        ],
    );
    r.mucc("r15", &["rm", "branches/b1/d"]);
    r.mucc("r16", &["mv", "trunk/a.txt", "trunk/renamed.txt"]);
    r
}

fn decode_bytes(bytes: &[u8], opts: &Options) -> Result<Ir, Error> {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("brygge-svndelta-{}-{n}.dump", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    let out = decode(&Source::DumpFile(path.clone()), opts);
    let _ = std::fs::remove_file(&path);
    out
}

fn decode_bytes_with(bytes: &[u8], limits: &Limits) -> Result<Ir, Error> {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("brygge-svndelta-{}-{n}.dump", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    let out = decode_with(&Source::DumpFile(path.clone()), &Options::default(), limits);
    let _ = std::fs::remove_file(&path);
    out
}

/// The tree after atom `idx` (replaying every op from the first atom): path -> (bytes, mode).
fn tree_at(ir: &Ir, idx: usize) -> BTreeMap<String, (Vec<u8>, u32)> {
    let mut tree: BTreeMap<String, (Vec<u8>, u32)> = BTreeMap::new();
    for atom in &ir.atoms[..=idx] {
        for op in &atom.ops {
            match op {
                PathOp::Add {
                    path, blob, mode, ..
                }
                | PathOp::Modify {
                    path, blob, mode, ..
                }
                | PathOp::Replace {
                    path, blob, mode, ..
                } => {
                    tree.insert(
                        path.clone(),
                        (ir.content.get(blob).unwrap().to_vec(), *mode),
                    );
                }
                PathOp::Delete { path, .. } => {
                    tree.remove(path);
                }
            }
        }
    }
    tree
}

/// Every revision's tree, contents and modes equal what Subversion itself says (`svnlook tree`, `svn cat`,
/// `svn proplist`), at every revision (handoff tests 1 and 6).
fn assert_matches_svn(ir: &Ir, repo: &Repo) {
    let youngest = repo.youngest();
    assert_eq!(
        ir.atoms.len() as u64,
        youngest + 1,
        "one atom per revision, r0 included"
    );
    for r in 0..=youngest {
        let tree = tree_at(ir, r as usize);
        let listing = String::from_utf8(run(Command::new("svnlook")
            .args(["tree", "--full-paths", "-r", &r.to_string()])
            .arg(repo.path())))
        .unwrap();
        let files: Vec<&str> = listing
            .lines()
            .map(|l| l.trim_start_matches('/'))
            .filter(|l| !l.is_empty() && !l.ends_with('/'))
            .collect();
        let got: Vec<&str> = tree.keys().map(String::as_str).collect();
        assert_eq!(got, files, "r{r}: the files");
        for path in files {
            let url = format!("{}/{path}@{r}", repo.url());
            let text = run(Command::new("svn").args(["cat", &url]));
            let props =
                String::from_utf8(run(Command::new("svn").args(["proplist", "-q", &url]))).unwrap();
            let names: Vec<&str> = props.lines().map(str::trim).collect();
            let (special, exec) = (
                names.contains(&"svn:special"),
                names.contains(&"svn:executable"),
            );
            let mode = if special {
                0o120_000
            } else if exec {
                0o100_755
            } else {
                0o100_644
            };
            let want = if special {
                text.strip_prefix(b"link ").unwrap().to_vec()
            } else {
                text
            };
            let (blob, m) = &tree[path];
            assert_eq!(blob, &want, "r{r} {path}: the content is `svn cat`'s");
            assert_eq!(
                *m, mode,
                "r{r} {path}: the mode follows svn:special and svn:executable"
            );
        }
    }
}

/// Handoff test 1: the fulltext dump, the `--deltas` dump and the `svnrdump` dump of one repository give
/// **byte-identical** artifacts, and every revision matches Subversion's own view.
#[test]
fn the_three_dump_forms_give_one_artifact_and_match_subversion() {
    if !svn_tools() {
        eprintln!("skipping: the subversion tools are not on PATH");
        return;
    }
    let repo = rich_repo();
    let opts = Options::default();
    let full = decode_bytes(&repo.dump(Form::Full), &opts).unwrap();
    let deltas_dump = repo.dump(Form::Deltas);
    let rdump = repo.dump(Form::Rdump);
    assert!(
        deltas_dump.windows(16).any(|w| w == b"Text-delta: true"),
        "the --deltas dump really carries text deltas"
    );
    assert!(
        rdump.windows(16).any(|w| w == b"Prop-delta: true"),
        "and svnrdump property deltas"
    );
    assert!(
        !rdump.windows(17).any(|w| w == b"Text-content-sha1"),
        "svnrdump gives MD5 only"
    );
    let deltas = decode_bytes(&deltas_dump, &opts).unwrap();
    let rdumped = decode_bytes(&rdump, &opts).unwrap();
    let want = brygge_ir::to_bytes(&full);
    assert_eq!(brygge_ir::to_bytes(&deltas), want, "fulltext and --deltas");
    assert_eq!(brygge_ir::to_bytes(&rdumped), want, "fulltext and svnrdump");
    assert_matches_svn(&full, &repo);

    // With branch/tag reconstruction on, the refs and the loss records agree too.
    let refs = Options {
        reconstruct_refs: true,
        ..Options::default()
    };
    let f = decode_bytes(&repo.dump(Form::Full), &refs).unwrap();
    let d = decode_bytes(&deltas_dump, &refs).unwrap();
    let rr = decode_bytes(&rdump, &refs).unwrap();
    assert_eq!(brygge_ir::to_bytes(&d), brygge_ir::to_bytes(&f));
    assert_eq!(brygge_ir::to_bytes(&rr), brygge_ir::to_bytes(&f));
}

/// Handoff test 6: `svn:special` set on a file with no text change, and cleared on a symlink with none. Content
/// and mode equal `svn cat` and `svn proplist` at each revision, in all three forms.
#[test]
fn setting_and_clearing_svn_special_without_text_follows_the_new_flag() {
    if !svn_tools() {
        eprintln!("skipping: the subversion tools are not on PATH");
        return;
    }
    let r = Repo::new();
    let t = r.file("t", b"link target\n");
    r.mucc(
        "r1 a plain file whose text starts with link",
        &["mkdir", "d", "put", &t, "d/f"],
    );
    r.mucc(
        "r2 becomes a symlink, no text",
        &["propset", "svn:special", "*", "d/f"],
    );
    r.mucc(
        "r3 executable too",
        &["propset", "svn:executable", "*", "d/f"],
    );
    r.mucc(
        "r4 no longer special, still executable",
        &["propdel", "svn:special", "d/f"],
    );
    r.mucc("r5 special again", &["propset", "svn:special", "*", "d/f"]);
    r.mucc(
        "r6 executable gone, special stays",
        &["propdel", "svn:executable", "d/f"],
    );
    r.mucc("r7 special gone", &["propdel", "svn:special", "d/f"]);
    for form in [Form::Full, Form::Deltas, Form::Rdump] {
        let ir = decode_bytes(&r.dump(form), &Options::default()).unwrap();
        assert_matches_svn(&ir, &r);
    }
}

/// A file made special without text whose SVN text is not `link <target>` is refused, as a full dump always
/// refused it: `Read`, never a guess.
#[test]
fn becoming_special_over_text_without_a_link_prefix_is_read() {
    if !svn_tools() {
        eprintln!("skipping: the subversion tools are not on PATH");
        return;
    }
    let r = Repo::new();
    let t = r.file("t", b"just text\n");
    r.mucc("r1", &["put", &t, "f"]);
    r.mucc("r2", &["propset", "svn:special", "*", "f"]);
    for form in [Form::Full, Form::Deltas, Form::Rdump] {
        match decode_bytes(&r.dump(form), &Options::default()) {
            Err(Error::Read(m)) => {
                assert!(m.contains("svn:special file without a link target"), "{m}")
            }
            other => panic!("expected Read, got {other:?}"),
        }
    }
}

/// Handoff test 5: an incremental delta dump cannot be decoded alone; the refusal is `Read`, names the path and
/// says why. Nothing is guessed.
#[test]
fn an_incremental_delta_dump_is_read_because_its_base_is_missing() {
    if !svn_tools() {
        eprintln!("skipping: the subversion tools are not on PATH");
        return;
    }
    let r = Repo::new();
    let a = r.file("a", b"one\ntwo\n");
    let a2 = r.file("a2", b"one\ntwo\nthree\n");
    r.mucc("r1", &["put", &a, "a.txt"]);
    r.mucc("r2", &["put", &a2, "a.txt"]);
    match decode_bytes(&r.dump(Form::IncrementalFrom(2)), &Options::default()) {
        Err(Error::Read(m)) => {
            assert!(m.contains("a.txt"), "{m}");
            assert!(
                m.contains("delta against a base not present in this dump"),
                "{m}"
            );
            assert!(m.contains("incremental or partial delta dump"), "{m}");
        }
        other => panic!("expected Read, got {other:?}"),
    }
}

/// Handoff test 4 against a real dump: one byte of a delta's new data flipped, so the target no longer has the
/// MD5 the dump states. `Read`, naming the header (the end-to-end check of our own delta application).
#[test]
fn a_flipped_byte_in_a_real_deltas_new_data_is_a_content_md5_mismatch() {
    if !svn_tools() {
        eprintln!("skipping: the subversion tools are not on PATH");
        return;
    }
    let r = Repo::new();
    let a = r.file(
        "a",
        b"a line that is long enough to be copied\nsecond line\n",
    );
    let a2 = r.file(
        "a2",
        b"a line that is long enough to be copied\nA-MARKER-NEW-LINE\n",
    );
    r.mucc("r1", &["put", &a, "a.txt"]);
    r.mucc("r2", &["put", &a2, "a.txt"]);
    for form in [Form::Deltas, Form::Rdump] {
        let mut dump = r.dump(form);
        let at = dump
            .windows(8)
            .position(|w| w == b"A-MARKER")
            .expect("the new data is in the delta");
        dump[at] = b'B';
        match decode_bytes(&dump, &Options::default()) {
            Err(Error::Read(m)) => {
                assert!(m.contains("a.txt: Text-content-md5 mismatch"), "{m}");
                assert!(
                    m.contains("the dump or its base is not what it states"),
                    "{m}"
                );
            }
            other => panic!("expected Read, got {other:?}"),
        }
    }
}

// ---- hand-written dumps (run everywhere) -----------------------------------------------------------------

/// One base-128 integer.
fn varint(mut n: u64) -> Vec<u8> {
    let mut groups = vec![(n & 0x7f) as u8];
    n >>= 7;
    while n > 0 {
        groups.push((n & 0x7f) as u8 | 0x80);
        n >>= 7;
    }
    groups.reverse();
    groups
}

/// An svndiff (version 0) of one window: `source` is the source view `(offset, len)`; `instructions` are
/// `(opcode, length, offset)`, and `new_data` the new-data section. The target length is what they produce.
fn svndiff(
    source: (u64, u64),
    instructions: &[(u8, usize, Option<u64>)],
    new_data: &[u8],
) -> Vec<u8> {
    let mut ins = Vec::new();
    let mut target_len = 0usize;
    for &(op, len, off) in instructions {
        target_len += len;
        if (1..64).contains(&len) {
            ins.push((op << 6) | len as u8);
        } else {
            ins.push(op << 6);
            ins.extend(varint(len as u64));
        }
        if let Some(o) = off {
            ins.extend(varint(o));
        }
    }
    let mut d = b"SVN\0".to_vec();
    for n in [
        source.0,
        source.1,
        target_len as u64,
        ins.len() as u64,
        new_data.len() as u64,
    ] {
        d.extend(varint(n));
    }
    d.extend(ins);
    d.extend(new_data);
    d
}

fn props_block(pairs: &[(&str, &str)], deletes: &[&str]) -> Vec<u8> {
    let mut b = Vec::new();
    for (k, v) in pairs {
        b.extend(format!("K {}\n{k}\nV {}\n{v}\n", k.len(), v.len()).into_bytes());
    }
    for k in deletes {
        b.extend(format!("D {}\n{k}\n", k.len()).into_bytes());
    }
    b.extend(b"PROPS-END\n");
    b
}

/// One node record. `extra` are further headers (checksums, `Text-delta`, ...), one per line.
fn node(
    path: &str,
    kind: &str,
    action: &str,
    extra: &[String],
    props: Option<Vec<u8>>,
    text: Option<&[u8]>,
) -> Vec<u8> {
    let mut n =
        format!("Node-path: {path}\nNode-kind: {kind}\nNode-action: {action}\n").into_bytes();
    for h in extra {
        n.extend(format!("{h}\n").into_bytes());
    }
    let (pl, tl) = (props.as_ref().map(Vec::len), text.map(<[u8]>::len));
    if let Some(p) = pl {
        n.extend(format!("Prop-content-length: {p}\n").into_bytes());
    }
    if let Some(t) = tl {
        n.extend(format!("Text-content-length: {t}\n").into_bytes());
    }
    if pl.is_some() || tl.is_some() {
        n.extend(format!("Content-length: {}\n", pl.unwrap_or(0) + tl.unwrap_or(0)).into_bytes());
    }
    n.push(b'\n');
    n.extend(props.unwrap_or_default());
    n.extend(text.unwrap_or_default());
    n.extend(b"\n\n");
    n
}

fn dump_of(revisions: &[Vec<Vec<u8>>]) -> Vec<u8> {
    let mut d =
        b"SVN-fs-dump-format-version: 3\n\nUUID: 11111111-2222-3333-4444-555555555555\n\n".to_vec();
    for (i, nodes) in revisions.iter().enumerate() {
        let p = props_block(
            &[
                ("svn:log", "x"),
                ("svn:date", "2024-01-01T00:00:00.000000Z"),
            ],
            &[],
        );
        d.extend(
            format!(
                "Revision-number: {i}\nProp-content-length: {0}\nContent-length: {0}\n\n",
                p.len()
            )
            .into_bytes(),
        );
        d.extend(p);
        d.extend(b"\n");
        for n in nodes {
            d.extend(n);
        }
    }
    d
}

fn md5h(text: &[u8]) -> String {
    md5_hex(text).unwrap()
}

/// r1 adds `f` by a delta against empty (a `svnrdump` add: MD5 only); r2 changes it by a delta against r1's
/// text: copy the first six bytes, new data, copy the last four.
fn two_delta_revisions(r2_extra: &[String]) -> Vec<u8> {
    let v1 = b"hello world\n";
    let d1 = svndiff((0, 0), &[(2, v1.len(), None)], v1);
    let v2 = b"hello, brave world\n"; // "hello" + ", brave" + " world\n"
    let d2 = svndiff(
        (0, 12),
        &[(0, 5, Some(0)), (2, 7, None), (0, 7, Some(5))],
        b", brave",
    );
    let mut e1 = vec![
        "Text-delta: true".to_string(),
        "Prop-delta: true".to_string(),
    ];
    e1.push(format!("Text-content-md5: {}", md5h(v1)));
    let mut e2 = vec!["Text-delta: true".to_string()];
    e2.push(format!("Text-delta-base-md5: {}", md5h(v1)));
    e2.push(format!("Text-content-md5: {}", md5h(v2)));
    e2.extend(r2_extra.iter().cloned());
    dump_of(&[
        vec![],
        vec![node(
            "f",
            "file",
            "add",
            &e1,
            Some(props_block(&[], &[])),
            Some(&d1),
        )],
        vec![node("f", "file", "change", &e2, None, Some(&d2))],
    ])
}

#[test]
fn a_text_delta_against_empty_and_against_the_previous_text_is_applied_and_checked() {
    let ir = decode_bytes(&two_delta_revisions(&[]), &Options::default()).unwrap();
    assert_eq!(tree_at(&ir, 1)["f"].0, b"hello world\n");
    assert_eq!(tree_at(&ir, 2)["f"].0, b"hello, brave world\n");
}

fn expect_read(dump: &[u8], needle: &str) {
    match decode_bytes(dump, &Options::default()) {
        Err(Error::Read(m)) => assert!(m.contains(needle), "{m}"),
        other => panic!("expected Read containing {needle:?}, got {other:?}"),
    }
}

/// Handoff test 4: each checksum header, wrong, is `Read` naming the header (and the path).
#[test]
fn a_wrong_checksum_header_is_read_naming_the_header() {
    let wrong = "0123456789abcdef0123456789abcdef".to_string();
    let v1 = b"hello world\n";
    let d1 = svndiff((0, 0), &[(2, v1.len(), None)], v1);
    let v2 = b"hello, brave world\n";
    let d2 = svndiff(
        (0, 12),
        &[(0, 5, Some(0)), (2, 7, None), (0, 7, Some(5))],
        b", brave",
    );
    let r1 = node(
        "f",
        "file",
        "add",
        &[
            "Text-delta: true".into(),
            format!("Text-content-md5: {}", md5h(v1)),
        ],
        Some(props_block(&[], &[])),
        Some(&d1),
    );
    let r2 = |extra: Vec<String>| {
        node(
            "f",
            "file",
            "change",
            &[vec!["Text-delta: true".to_string()], extra].concat(),
            None,
            Some(&d2),
        )
    };
    let dump = |n2: Vec<u8>| dump_of(&[vec![], vec![r1.clone()], vec![n2]]);

    expect_read(
        &dump(r2(vec![
            format!("Text-delta-base-md5: {wrong}"),
            format!("Text-content-md5: {}", md5h(v2)),
        ])),
        "f: Text-delta-base-md5 mismatch",
    );
    expect_read(
        &dump(r2(vec![format!("Text-content-md5: {wrong}")])),
        "f: Text-content-md5 mismatch",
    );
    expect_read(
        &dump(r2(vec![format!("Text-content-sha1: {}", "0".repeat(40))])),
        "f: Text-content-sha1 mismatch",
    );
    expect_read(
        &dump(r2(vec![format!(
            "Text-delta-base-sha1: {}",
            "0".repeat(40)
        )])),
        "f: Text-delta-base-sha1 mismatch",
    );
    // the right ones pass, sha1 included
    let ok = dump(r2(vec![
        format!("Text-delta-base-md5: {}", md5h(v1)),
        format!("Text-delta-base-sha1: {}", sha1_hex(v1).unwrap()),
        format!("Text-content-md5: {}", md5h(v2)),
        format!("Text-content-sha1: {}", sha1_hex(v2).unwrap()),
    ]));
    assert_eq!(
        tree_at(&decode_bytes(&ok, &Options::default()).unwrap(), 2)["f"].0,
        v2
    );
}

/// New behaviour: a **fulltext** node with a wrong `Text-content-md5` (or `-sha1`) is `Read`, too.
#[test]
fn a_fulltext_node_with_a_wrong_content_checksum_is_read() {
    let text = b"plain fulltext\n";
    let mk = |extra: String| {
        dump_of(&[
            vec![],
            vec![node(
                "f",
                "file",
                "add",
                &[extra],
                Some(props_block(&[], &[])),
                Some(text),
            )],
        ])
    };
    let ok = mk(format!("Text-content-md5: {}", md5h(text)));
    assert_eq!(
        tree_at(&decode_bytes(&ok, &Options::default()).unwrap(), 1)["f"].0,
        text
    );
    expect_read(
        &mk("Text-content-md5: 00000000000000000000000000000000".into()),
        "f: Text-content-md5 mismatch",
    );
    expect_read(
        &mk(format!("Text-content-sha1: {}", "1".repeat(40))),
        "f: Text-content-sha1 mismatch",
    );
}

/// A copy with text states its source's checksum: checked against the copy source's text.
#[test]
fn a_copy_sources_checksum_is_checked() {
    let v1 = b"source text\n";
    let v2 = b"copied and edited\n";
    let mk = |copy_md5: String| {
        dump_of(&[
            vec![],
            vec![node(
                "a",
                "file",
                "add",
                &[],
                Some(props_block(&[], &[])),
                Some(v1),
            )],
            vec![{
                let mut h = vec![
                    "Node-copyfrom-rev: 1".to_string(),
                    "Node-copyfrom-path: a".to_string(),
                ];
                h.push(format!("Text-copy-source-md5: {copy_md5}"));
                h.push(format!("Text-content-md5: {}", md5h(v2)));
                node(
                    "b",
                    "file",
                    "add",
                    &h,
                    Some(props_block(&[], &[])),
                    Some(v2),
                )
            }],
        ])
    };
    assert!(decode_bytes(&mk(md5h(v1)), &Options::default()).is_ok());
    expect_read(&mk("f".repeat(32)), "b: Text-copy-source-md5 mismatch");
}

#[test]
fn a_delta_against_a_path_that_is_not_there_is_read_naming_the_path() {
    let d = svndiff((0, 0), &[(2, 3, None)], b"abc");
    let dump = dump_of(&[
        vec![],
        vec![node(
            "gone",
            "file",
            "change",
            &["Text-delta: true".into()],
            None,
            Some(&d),
        )],
    ]);
    expect_read(&dump, "gone: delta against a base not present in this dump");
}

#[test]
fn svndiff_versions_1_and_2_are_refused_by_name_and_say_how_to_redump() {
    for (v, name) in [(1u8, "version 1 (zlib)"), (2, "version 2 (lz4)")] {
        let body = [b'S', b'V', b'N', v, 0, 0, 0, 0, 0];
        let dump = dump_of(&[
            vec![],
            vec![node(
                "f",
                "file",
                "add",
                &["Text-delta: true".into()],
                None,
                Some(&body),
            )],
        ]);
        match decode_bytes(&dump, &Options::default()) {
            Err(Error::UnsupportedFormat { what, reason }) => {
                assert!(what.contains(name), "{what}");
                assert!(reason.contains("svnadmin dump"), "{reason}");
                assert!(reason.contains("svnrdump"), "{reason}");
            }
            other => panic!("expected UnsupportedFormat, got {other:?}"),
        }
    }
}

#[test]
fn a_malformed_delta_names_the_node() {
    let mut bad = svndiff((0, 0), &[(2, 3, None)], b"abc");
    bad.truncate(bad.len() - 1); // one byte of new data short
    let dump = dump_of(&[
        vec![],
        vec![node(
            "dir/f",
            "file",
            "add",
            &["Text-delta: true".into()],
            None,
            Some(&bad),
        )],
    ]);
    expect_read(&dump, "dir/f: text delta");
    // a body that is not svndiff at all, and one shorter than the header
    for body in [&b"HELLO"[..], b"SV"] {
        let dump = dump_of(&[
            vec![],
            vec![node(
                "f",
                "file",
                "add",
                &["Text-delta: true".into()],
                None,
                Some(body),
            )],
        ]);
        expect_read(&dump, "f: the text delta is not svndiff");
    }
}

/// A property delta: `K` sets, `D` removes, against the base's properties. The mode flags follow: a file with
/// both `svn:special` and `svn:executable` keeps being executable when `svn:special` is deleted.
#[test]
fn a_property_delta_sets_and_deletes_against_the_bases_flags() {
    let t = b"link target";
    let both = props_block(&[("svn:special", "*"), ("svn:executable", "*")], &[]);
    let dump = dump_of(&[
        vec![],
        vec![node("f", "file", "add", &[], Some(both), Some(t))],
        vec![node(
            "f",
            "file",
            "change",
            &["Prop-delta: true".into()],
            Some(props_block(&[], &["svn:special"])),
            None,
        )],
        vec![node(
            "f",
            "file",
            "change",
            &["Prop-delta: true".into()],
            Some(props_block(&[("svn:special", "*")], &["svn:executable"])),
            None,
        )],
    ]);
    let ir = decode_bytes(&dump, &Options::default()).unwrap();
    assert_eq!(
        tree_at(&ir, 1)["f"],
        (b"target".to_vec(), 0o120_000),
        "special wins over executable"
    );
    // `svn:special` deleted: still executable, and the text is SVN's (`link target`), not the bare target
    assert_eq!(tree_at(&ir, 2)["f"], (b"link target".to_vec(), 0o100_755));
    // special set again and executable deleted: a symlink, the target out of `link target`
    assert_eq!(tree_at(&ir, 3)["f"], (b"target".to_vec(), 0o120_000));
}

#[test]
fn a_property_delta_of_a_copy_starts_from_the_copy_sources_properties() {
    let dump = dump_of(&[
        vec![],
        vec![node(
            "a",
            "file",
            "add",
            &[],
            Some(props_block(&[("svn:executable", "*")], &[])),
            Some(b"x"),
        )],
        vec![node(
            "b",
            "file",
            "add",
            &[
                "Node-copyfrom-rev: 1".into(),
                "Node-copyfrom-path: a".into(),
                "Prop-delta: true".into(),
            ],
            Some(props_block(&[("color", "red")], &[])),
            None,
        )],
    ]);
    let ir = decode_bytes(&dump, &Options::default()).unwrap();
    assert_eq!(
        tree_at(&ir, 2)["b"],
        (b"x".to_vec(), 0o100_755),
        "the copy keeps the source's executable bit"
    );
}

/// A `D` entry in a property block that is not a delta is malformed; and a delta block's entries are the only
/// ones classified and refused (`svn:externals` set by a delta is refused exactly as by a full block).
#[test]
fn a_deletion_in_a_full_block_is_read_and_externals_in_a_delta_are_refused() {
    let dump = dump_of(&[
        vec![],
        vec![node(
            "f",
            "file",
            "add",
            &[],
            Some(props_block(&[], &["custom"])),
            Some(b"x"),
        )],
    ]);
    expect_read(&dump, "not a delta");
    let dump = dump_of(&[
        vec![],
        vec![node(
            "d",
            "dir",
            "add",
            &["Prop-delta: true".into()],
            Some(props_block(&[("svn:externals", "x http://e")], &[])),
            None,
        )],
    ]);
    match decode_bytes(&dump, &Options::default()) {
        Err(Error::FloorRefusal { feature, .. }) => assert_eq!(feature, crate::floor::EXTERNALS),
        other => panic!("expected the externals refusal, got {other:?}"),
    }
}

/// Handoff test 7: small `Limits` give `ResourceLimit` for the per-node and the total ceilings.
#[test]
fn the_per_node_and_the_total_ceilings_are_resource_limit() {
    let big = vec![b'z'; 300];
    // a delta that reconstructs 300 bytes from a few instructions: a run of `z`
    let d = svndiff((0, 0), &[(2, 1, None), (1, 299, Some(0))], b"z");
    let one = dump_of(&[
        vec![],
        vec![node(
            "f",
            "file",
            "add",
            &["Text-delta: true".into()],
            None,
            Some(&d),
        )],
    ]);
    let ok = Limits {
        max_dump_bytes: 10_000,
        max_node_text_bytes: 300,
        ..Limits::default()
    };
    let ir = decode_bytes_with(&one, &ok).unwrap();
    assert_eq!(tree_at(&ir, 1)["f"].0, big);

    // per node: 299 is one byte under what the node needs
    let node_cap = Limits {
        max_node_text_bytes: 299,
        ..ok
    };
    match decode_bytes_with(&one, &node_cap) {
        Err(Error::ResourceLimit { what, ceiling }) => {
            assert!(what.contains("'f'"), "{what}");
            assert_eq!(ceiling, "299 bytes");
        }
        other => panic!("expected ResourceLimit, got {other:?}"),
    }
    // total: three such nodes fit a node ceiling of 300 but not a total of 800
    let three = dump_of(&[
        vec![],
        vec![
            node(
                "a",
                "file",
                "add",
                &["Text-delta: true".into()],
                None,
                Some(&d),
            ),
            node(
                "b",
                "file",
                "add",
                &["Text-delta: true".into()],
                None,
                Some(&d),
            ),
            node(
                "c",
                "file",
                "add",
                &["Text-delta: true".into()],
                None,
                Some(&d),
            ),
        ],
    ]);
    let total_cap = Limits {
        max_dump_bytes: 800,
        ..ok
    };
    match decode_bytes_with(&three, &total_cap) {
        Err(Error::ResourceLimit { what, ceiling }) => {
            assert!(what.contains("in total"), "{what}");
            assert_eq!(ceiling, "800 bytes");
        }
        other => panic!("expected ResourceLimit, got {other:?}"),
    }
    assert!(
        decode_bytes_with(
            &three,
            &Limits {
                max_dump_bytes: 900,
                ..ok
            }
        )
        .is_ok()
    );
    // a fulltext node over the per-node ceiling is refused too (fulltext or delta)
    let full = dump_of(&[
        vec![],
        vec![node(
            "f",
            "file",
            "add",
            &[],
            Some(props_block(&[], &[])),
            Some(&big),
        )],
    ]);
    assert!(matches!(
        decode_bytes_with(&full, &node_cap),
        Err(Error::ResourceLimit { .. })
    ));
}

/// The tree's entry type carries the two flags separately (a file can be both special and executable).
#[test]
fn a_file_entry_keeps_both_flags() {
    let e = FileEntry {
        blob: brygge_ir::BlobId::of(b"x"),
        mode: 0o120_000,
        exec: true,
        special: true,
    };
    assert!(e.exec && e.special);
}
