//! Tests for the revlog reader (RFC 005), validated against **ground truth**: a real Mercurial repo
//! built with the `hg` CLI, then every reconstructed revision compared byte-for-byte against
//! `hg debugdata` and every node against `hg log`. Skips if `hg` is not on `PATH`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{Limits, Revlog};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn hg_available() -> bool {
    Command::new("hg")
        .arg("--version")
        .env("HGRCPATH", "/dev/null")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

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
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("brygge-hg-test-{}-{nanos}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let r = Self { dir };
        r.run(&["init", r.dir.to_str().unwrap()]);
        r
    }

    fn store(&self) -> PathBuf {
        self.dir.join(".hg").join("store")
    }

    fn run(&self, args: &[&str]) -> Vec<u8> {
        let out = Command::new("hg")
            .env("HGRCPATH", "/dev/null")
            .arg("--cwd")
            .arg(&self.dir)
            .arg("-R")
            .arg(&self.dir)
            .arg("--config")
            .arg("ui.username=A U Thor <a@example.com>")
            .args(args)
            .output()
            .expect("run hg");
        assert!(
            out.status.success(),
            "hg {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }

    fn write(&self, rel: &str, contents: &str) {
        std::fs::write(self.dir.join(rel), contents).unwrap();
    }

    fn commit(&self, epoch: &str, msg: &str) {
        self.run(&["add"]);
        self.run(&["commit", "-d", &format!("{epoch} 0"), "-m", msg]);
    }
}

fn hex20(node: &[u8; 20]) -> String {
    crate::util::hex(node)
}

/// A repo exercising every reader path: a small ('u') file, a large compressible (zstd) file, a delta
/// (append), a modification, and a rename (`hg mv`, filelog copy metadata).
fn build_repo() -> Repo {
    let r = Repo::new();
    r.write("readme.txt", "hello\n");
    r.write("big.txt", &"abcdefgh\n".repeat(4000)); // large + compressible -> zstd chunk
    r.commit("1136239445", "initial");

    r.write("readme.txt", "hello world\n"); // modify
    r.write("big.txt", &("abcdefgh\n".repeat(4000) + "one more line\n")); // append -> delta
    r.commit("1136239446", "grow");

    r.run(&["mv", "readme.txt", "docs.txt"]); // rename -> filelog copy metadata
    r.commit("1136239447", "rename readme");
    r
}

/// Compare every revision of one revlog against `hg debugdata`.
fn assert_revlog_matches(
    r: &Repo,
    index_path: &Path,
    debugdata_args: &dyn Fn(usize) -> Vec<String>,
) {
    let rl = Revlog::open(index_path).expect("open revlog");
    assert!(!rl.is_empty(), "revlog {index_path:?} has no revisions");
    for rev in 0..rl.len() {
        let mine = rl.revision(rev).expect("reconstruct revision");
        let args: Vec<String> = debugdata_args(rev);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let truth = r.run(&refs);
        assert_eq!(
            mine, truth,
            "revision {rev} of {index_path:?} does not match hg debugdata"
        );
    }
}

#[test]
fn changelog_manifest_and_filelogs_match_hg_debugdata() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = build_repo();
    let store = r.store();

    // Changelog (split: 00changelog.i + .d) — full snapshots and any deltas.
    assert_revlog_matches(&r, &store.join("00changelog.i"), &|rev| {
        vec!["debugdata".into(), "-c".into(), rev.to_string()]
    });

    // Manifest (inline).
    assert_revlog_matches(&r, &store.join("00manifest.i"), &|rev| {
        vec!["debugdata".into(), "-m".into(), rev.to_string()]
    });

    // Filelogs: big.txt exercises a zstd snapshot (rev0) + a delta (rev1); readme/docs exercise the
    // rename's copy-metadata header (returned verbatim in the revision text).
    for (path, store_name) in [
        ("big.txt", "data/big.txt.i"),
        ("readme.txt", "data/readme.txt.i"),
        ("docs.txt", "data/docs.txt.i"),
    ] {
        assert_revlog_matches(&r, &store.join(store_name), &move |rev| {
            vec!["debugdata".into(), path.to_string(), rev.to_string()]
        });
    }
}

#[test]
fn nodes_and_parents_match_hg_log() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = build_repo();
    let rl = Revlog::open(&r.store().join("00changelog.i")).expect("open changelog");
    assert_eq!(rl.len(), 3, "three changesets");
    for rev in 0..rl.len() {
        let entry = rl.entry(rev).expect("entry");
        let truth =
            String::from_utf8(r.run(&["log", "-r", &rev.to_string(), "-T", "{node}"])).unwrap();
        assert_eq!(hex20(&entry.node), truth.trim(), "changelog node {rev}");
    }
    // The first changeset is a root (both parents null); the second's p1 is rev 0.
    assert_eq!(rl.entry(0).unwrap().p1, super::NULL_REV);
    assert_eq!(rl.entry(1).unwrap().p1, 0);
}

#[test]
fn a_big_file_uses_zstd_and_a_delta_that_reconstruct() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let r = build_repo();
    let rl = Revlog::open(&r.store().join("data/big.txt.i")).expect("open big.txt filelog");
    assert_eq!(rl.len(), 2, "two file revisions");
    // rev1 is a delta on rev0; reconstructing it must reproduce the appended content exactly.
    let rev1 = rl.revision(1).expect("reconstruct delta revision");
    let expected = "abcdefgh\n".repeat(4000) + "one more line\n";
    assert_eq!(
        rev1,
        expected.as_bytes(),
        "delta+zstd reconstruction is exact"
    );
}

// ---- RFC 010 CR-17: decompression and mpatch ceilings, and the index length check -----------------

fn unique_dir(label: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "brygge-hg-revlog-{label}-{}-{n}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Hand-build a minimal split (non-inline) revlog holding one full-snapshot revision, with a chosen
/// stored chunk and a chosen (possibly wrong) recorded uncompressed length — so a ceiling or a
/// length-mismatch can be exercised directly, without needing `hg` to produce a malformed store.
fn write_single_rev_revlog(dir: &Path, name: &str, chunk: &[u8], uncomp_len: u32) -> PathBuf {
    let mut entry = vec![0u8; 64];
    entry[0..4].copy_from_slice(&1u32.to_be_bytes()); // format version 1, not inline
    entry[8..12].copy_from_slice(&u32::try_from(chunk.len()).unwrap().to_be_bytes()); // comp_len
    entry[12..16].copy_from_slice(&uncomp_len.to_be_bytes());
    // base_rev (bytes 16..20) left at 0 == rev 0 -> a full snapshot.
    entry[24..28].copy_from_slice(&(-1i32).to_be_bytes()); // p1 = NULL_REV
    entry[28..32].copy_from_slice(&(-1i32).to_be_bytes()); // p2 = NULL_REV
    let index_path = dir.join(format!("{name}.i"));
    std::fs::write(&index_path, &entry).unwrap();
    std::fs::write(dir.join(format!("{name}.d")), chunk).unwrap();
    index_path
}

#[test]
fn a_zlib_chunk_that_inflates_past_the_limit_is_refused() {
    use std::io::Write as _;
    let payload = vec![0u8; 200_000]; // highly compressible
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(&payload).unwrap();
    let chunk = enc.finish().unwrap();
    assert_eq!(
        chunk.first(),
        Some(&b'x'),
        "a zlib stream's first byte is the 'x' marker"
    );

    let dir = unique_dir("zlib-bomb");
    let index_path = write_single_rev_revlog(&dir, "rev", &chunk, payload.len() as u32);
    let rl = Revlog::open(&index_path).expect("open revlog");
    let limits = Limits {
        max_revision_bytes: 1024,
    };
    let result = rl.revision_with(0, &limits);
    let _ = std::fs::remove_dir_all(&dir);
    match result {
        Err(crate::Error::ResourceLimit { .. }) => {}
        other => panic!("expected a resource-limit refusal, got {other:?}"),
    }
}

fn zstd_available() -> bool {
    Command::new("zstd")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn a_zstd_chunk_that_inflates_past_the_limit_is_refused() {
    if !zstd_available() {
        eprintln!("skipping: zstd not available");
        return;
    }
    use std::io::Write as _;
    let payload = vec![1u8; 200_000]; // highly compressible
    let mut child = Command::new("zstd")
        .args(["-q", "-c"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn zstd");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(&payload)
        .unwrap();
    let output = child.wait_with_output().expect("zstd output");
    assert!(output.status.success(), "zstd compression failed");
    let chunk = output.stdout;
    assert_eq!(
        chunk.first(),
        Some(&0x28),
        "a zstd frame's first byte is the 0x28 marker"
    );

    let dir = unique_dir("zstd-bomb");
    let index_path = write_single_rev_revlog(&dir, "rev", &chunk, payload.len() as u32);
    let rl = Revlog::open(&index_path).expect("open revlog");
    let limits = Limits {
        max_revision_bytes: 1024,
    };
    let result = rl.revision_with(0, &limits);
    let _ = std::fs::remove_dir_all(&dir);
    match result {
        Err(crate::Error::ResourceLimit { .. }) => {}
        other => panic!("expected a resource-limit refusal, got {other:?}"),
    }
}

#[test]
fn a_snapshot_length_mismatch_against_the_index_is_a_read_error() {
    // Marker 0x00 is raw (byte 0 included), so the reconstructed text is exactly this chunk.
    let mut chunk = vec![0x00];
    chunk.extend_from_slice(b"hello world");
    let claimed_uncomp_len = 999; // deliberately wrong: the real length is chunk.len() == 12

    let dir = unique_dir("length-mismatch");
    let index_path = write_single_rev_revlog(&dir, "rev", &chunk, claimed_uncomp_len);
    let rl = Revlog::open(&index_path).expect("open revlog");
    let result = rl.revision(0);
    let _ = std::fs::remove_dir_all(&dir);
    match result {
        Err(crate::Error::Read(msg)) => {
            assert!(
                msg.contains("does not match the index"),
                "unexpected message: {msg}"
            );
        }
        other => panic!("expected Error::Read, got {other:?}"),
    }
}

// ---- release-prep §1: every revision is verified against its node ---------------------------------------

/// Mercurial's revision hash, written out independently of the reader under test:
/// `sha1(min(p1,p2) ‖ max(p1,p2) ‖ text)` with the null parent as 20 zero bytes
/// (`utils/storageutil.py` `hashrevisionsha1`).
fn hg_node(text: &[u8], p1: [u8; 20], p2: [u8; 20]) -> [u8; 20] {
    use sha1_checked::{Digest, Sha1};
    let (a, b) = if p1 < p2 { (p1, p2) } else { (p2, p1) };
    let mut h = Sha1::new();
    Digest::update(&mut h, a);
    Digest::update(&mut h, b);
    Digest::update(&mut h, text);
    h.finalize().into()
}

/// One hand-built revision: the text it is *stored* with, the text its node is *computed* from, its
/// parents (revision numbers within the same revlog, `-1` for null) and flags.
struct HandRev<'a> {
    stored: &'a [u8],
    hashed: &'a [u8],
    p1: i32,
    p2: i32,
    flags: u16,
}

impl<'a> HandRev<'a> {
    fn honest(text: &'a [u8], p1: i32, p2: i32) -> Self {
        Self {
            stored: text,
            hashed: text,
            p1,
            p2,
            flags: 0,
        }
    }
}

/// Hand-build a split revlog of full-snapshot revisions with **correct** nodes (except where a `HandRev`
/// stores different text than it hashed). Returns the index path and every revision's node.
fn write_revlog(dir: &Path, name: &str, revs: &[HandRev<'_>]) -> (PathBuf, Vec<[u8; 20]>) {
    let mut nodes: Vec<[u8; 20]> = Vec::new();
    let mut index = Vec::new();
    let mut data = Vec::new();
    for (i, r) in revs.iter().enumerate() {
        let node_of = |p: i32| -> [u8; 20] {
            if p < 0 {
                [0u8; 20]
            } else {
                nodes[usize::try_from(p).unwrap()]
            }
        };
        let node = hg_node(r.hashed, node_of(r.p1), node_of(r.p2));
        let mut chunk = vec![b'u'];
        chunk.extend_from_slice(r.stored);
        let mut entry = vec![0u8; 64];
        if i == 0 {
            entry[0..4].copy_from_slice(&1u32.to_be_bytes()); // format version 1, not inline
        }
        entry[6..8].copy_from_slice(&r.flags.to_be_bytes());
        entry[8..12].copy_from_slice(&u32::try_from(chunk.len()).unwrap().to_be_bytes());
        entry[12..16].copy_from_slice(&u32::try_from(r.stored.len()).unwrap().to_be_bytes());
        entry[16..20].copy_from_slice(&i32::try_from(i).unwrap().to_be_bytes()); // full snapshot
        entry[20..24].copy_from_slice(&i32::try_from(i).unwrap().to_be_bytes()); // link_rev
        entry[24..28].copy_from_slice(&r.p1.to_be_bytes());
        entry[28..32].copy_from_slice(&r.p2.to_be_bytes());
        entry[32..52].copy_from_slice(&node);
        index.extend_from_slice(&entry);
        data.extend_from_slice(&chunk);
        nodes.push(node);
    }
    let index_path = dir.join(format!("{name}.i"));
    std::fs::write(&index_path, &index).unwrap();
    std::fs::write(dir.join(format!("{name}.d")), &data).unwrap();
    (index_path, nodes)
}

fn hex(node: &[u8; 20]) -> String {
    crate::util::hex(node)
}

#[test]
fn honest_revisions_verify_including_a_merge_and_either_parent_order() {
    let dir = unique_dir("verify-ok");
    let (path, _) = write_revlog(
        &dir,
        "ok",
        &[
            HandRev::honest(b"root\n", -1, -1),
            HandRev::honest(b"child\n", 0, -1),
            // Mercurial's filelog stores a has-meta revision as (null, p) as well as (p, null): the hash
            // is symmetric in its parents, and both orders must verify.
            HandRev::honest(b"swapped\n", -1, 1),
            HandRev::honest(b"merge\n", 1, 2),
            HandRev::honest(b"merge, other order\n", 2, 1),
        ],
    );
    let rl = Revlog::open(&path).unwrap();
    for rev in 0..rl.len() {
        rl.revision(rev)
            .unwrap_or_else(|e| panic!("rev {rev} should verify: {e:?}"));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn text_altered_without_updating_the_node_is_refused_naming_the_node() {
    let dir = unique_dir("verify-altered");
    let (path, nodes) = write_revlog(
        &dir,
        "bad",
        &[
            HandRev::honest(b"root\n", -1, -1),
            HandRev {
                stored: b"tampered\n",
                hashed: b"original\n",
                p1: 0,
                p2: -1,
                flags: 0,
            },
        ],
    );
    let rl = Revlog::open(&path).unwrap();
    rl.revision(0)
        .expect("the untouched revision still verifies");
    match rl.revision(1) {
        Err(crate::Error::Read(msg)) => {
            assert_eq!(
                msg,
                format!(
                    "revision {} does not match its content (corrupt or crafted store)",
                    hex(&nodes[1])
                )
            );
        }
        other => panic!("expected the exact mismatch Read error, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_wrong_parent_changes_the_hash_and_is_refused() {
    // The node commits to the parents too: the same text under a different parent does not verify.
    let dir = unique_dir("verify-parent");
    let (path, _) = write_revlog(&dir, "p", &[HandRev::honest(b"a\n", -1, -1)]);
    let mut index = std::fs::read(&path).unwrap();
    // Append a second revision whose node was computed with the root as p1, but whose index entry (copied
    // from the root's) claims null parents.
    let mut second = index[..64].to_vec();
    second[0..4].copy_from_slice(&[0, 0, 0, 0]);
    let root_node: [u8; 20] = index[32..52].try_into().unwrap();
    second[32..52].copy_from_slice(&hg_node(b"a\n", root_node, [0u8; 20]));
    second[16..20].copy_from_slice(&1i32.to_be_bytes());
    index.extend_from_slice(&second); // p1/p2 stay -1 (copied from the root entry)
    std::fs::write(&path, &index).unwrap();
    let mut data = std::fs::read(dir.join("p.d")).unwrap();
    data.extend_from_slice(b"ua\n");
    std::fs::write(dir.join("p.d"), &data).unwrap();
    let rl = Revlog::open(&path).unwrap();
    rl.revision(0).unwrap();
    match rl.revision(1) {
        Err(crate::Error::Read(msg)) => {
            assert!(msg.contains("does not match its content"), "{msg}")
        }
        other => panic!("expected a mismatch, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_parent_revision_out_of_range_is_a_read_error_not_a_panic() {
    let dir = unique_dir("verify-oob");
    let (path, _) = write_revlog(&dir, "oob", &[HandRev::honest(b"a\n", -1, -1)]);
    let mut index = std::fs::read(&path).unwrap();
    index[24..28].copy_from_slice(&7i32.to_be_bytes()); // p1 = rev 7, which does not exist
    std::fs::write(&path, &index).unwrap();
    let rl = Revlog::open(&path).unwrap();
    assert!(matches!(rl.revision(0), Err(crate::Error::Read(_))));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_filelog_revision_with_copy_metadata_verifies_because_the_header_is_hashed() {
    // A filelog revision's raw text starts with `\1\ncopy: …\ncopyrev: …\n\1\n`; the node hashes all of it.
    let dir = unique_dir("verify-meta");
    let with_header: &[u8] =
        b"\x01\ncopy: old.txt\ncopyrev: 0123456789abcdef0123456789abcdef01234567\n\x01\ncontent\n";
    let (path, nodes) = write_revlog(&dir, "meta", &[HandRev::honest(with_header, -1, -1)]);
    let rl = Revlog::open(&path).unwrap();
    assert_eq!(rl.revision(0).unwrap(), with_header);

    // ... and a node computed over the header-stripped text does NOT verify the stored header'd text.
    let stripped: &[u8] = b"content\n";
    assert_ne!(nodes[0], hg_node(stripped, [0; 20], [0; 20]));
    let dir2 = unique_dir("verify-meta-stripped");
    let (path2, _) = write_revlog(
        &dir2,
        "meta",
        &[HandRev {
            stored: with_header,
            hashed: stripped,
            p1: -1,
            p2: -1,
            flags: 0,
        }],
    );
    let rl2 = Revlog::open(&path2).unwrap();
    assert!(matches!(rl2.revision(0), Err(crate::Error::Read(_))));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dir2);
}

// The revision-flag table (release-prep §1): which flags still verify, which are refused.

fn flagged(flags: u16) -> Result<Vec<u8>, crate::Error> {
    let dir = unique_dir("flags");
    let (path, _) = write_revlog(
        &dir,
        "f",
        &[HandRev {
            stored: b"text\n",
            hashed: b"text\n",
            p1: -1,
            p2: -1,
            flags,
        }],
    );
    let rl = Revlog::open(&path).unwrap();
    let r = rl.revision(0);
    let _ = std::fs::remove_dir_all(&dir);
    r
}

#[test]
fn the_hash_neutral_flags_verify_as_normal() {
    for (name, flag) in [
        ("has-copies-info", 1u16 << 12),
        ("has-meta", 1 << 11),
        ("delta-is-snapshot", 1 << 10),
        ("delta-has-quality", 1 << 9),
        ("delta-is-good", 1 << 8),
        ("delta-p1-small", 1 << 7),
        ("delta-p2-small", 1 << 6),
    ] {
        assert_eq!(
            flagged(flag).unwrap(),
            b"text\n",
            "flag {name} should verify"
        );
    }
    assert_eq!(
        flagged((1 << 12) | (1 << 11) | (1 << 10)).unwrap(),
        b"text\n"
    );
}

fn assert_refused(flags: u16, feature: &str) {
    match flagged(flags) {
        Err(crate::Error::FloorRefusal { feature: f, .. }) => assert_eq!(f, feature),
        other => panic!("flags {flags:#06x}: expected FloorRefusal({feature}), got {other:?}"),
    }
}

#[test]
fn flags_whose_stored_text_is_not_the_hashed_text_are_refused_by_name() {
    assert_refused(1 << 15, "censored-revision");
    assert_refused(1 << 14, "ellipsis-revision");
    assert_refused(1 << 13, "external-storage-revision");
    // A neutral flag does not rescue a refused one.
    assert_refused((1 << 14) | (1 << 11), "ellipsis-revision");
}

#[test]
fn a_flag_bit_mercurial_does_not_define_is_refused() {
    for bit in 0..6u16 {
        assert_refused(1 << bit, "unknown-revision-flag");
    }
}

// ---- review 012 F-2: the collision path and the ordinary path through the same helper ----------------

const SHAMBLES: &[u8] = include_bytes!("testdata/sha-mbles-1.bin");

fn arr20(b: &[u8]) -> [u8; 20] {
    b.try_into().unwrap()
}

#[test]
fn a_sha1_collision_input_is_refused_with_the_exact_collision_error() {
    // The SHAmbles vector, split so it is fed to the hash as `lo ‖ hi ‖ text` (the real chosen-prefix
    // collision block), exactly as a crafted revision would present it.
    assert_eq!(SHAMBLES.len(), 640);
    let lo = arr20(&SHAMBLES[0..20]);
    let hi = arr20(&SHAMBLES[20..40]);
    let text = &SHAMBLES[40..];
    let node = [0xabu8; 20];
    let err = super::check_revision_hash(&node, &lo, &hi, text).unwrap_err();
    let expected = format!(
        "revision {} triggers SHA-1 collision detection (crafted store)",
        crate::util::hex(&node)
    );
    assert!(
        matches!(&err, crate::Error::Read(m) if *m == expected),
        "expected the exact collision error, got {err:?}"
    );
}

#[test]
fn an_ordinary_input_passes_through_the_same_helper_and_a_wrong_node_does_not() {
    use sha1_checked::{Digest, Sha1};
    let lo = [1u8; 20];
    let hi = [2u8; 20];
    let text = b"ordinary revision text";
    let mut h = Sha1::new();
    Digest::update(&mut h, lo);
    Digest::update(&mut h, hi);
    Digest::update(&mut h, text);
    let node: [u8; 20] = h.finalize().into();
    super::check_revision_hash(&node, &lo, &hi, text).unwrap();

    let wrong = [0u8; 20];
    let err = super::check_revision_hash(&wrong, &lo, &hi, text).unwrap_err();
    let expected = format!(
        "revision {} does not match its content (corrupt or crafted store)",
        crate::util::hex(&wrong)
    );
    assert!(matches!(&err, crate::Error::Read(m) if *m == expected));
}

// ---- RFC 013 D-1: an absent file is an error only when the revlog needs it -----------------------------------

#[test]
fn an_absent_data_file_is_a_read_error_when_the_index_stores_data() {
    let dir = unique_dir("absent-d-needed");
    let index = write_single_rev_revlog(&dir, "f", b"u hello", 6);
    std::fs::remove_file(dir.join("f.d")).unwrap();
    match Revlog::open_with_data(&index, &dir.join("f.d"), None, "the data file of f") {
        Err(crate::Error::Read(m)) => {
            assert!(m.contains("the data file of f not found at"), "{m}");
            assert!(m.contains("f.d"), "names the file: {m}");
        }
        other => panic!("expected a Read error, got {:?}", other.map(|_| ())),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_absent_data_file_is_nothing_to_read_when_every_entry_is_empty() {
    let dir = unique_dir("absent-d-empty");
    // one revision whose stored chunk is empty (comp_len 0): the index says everything there is to say.
    let index = write_single_rev_revlog(&dir, "f", b"", 0);
    std::fs::remove_file(dir.join("f.d")).unwrap();
    let rl = Revlog::open_with_data(&index, &dir.join("f.d"), None, "the data file").unwrap();
    assert_eq!(rl.len(), 1, "the revlog reads as its index says");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_a_real_not_found_of_the_index_says_not_found() {
    let dir = unique_dir("absent-i");
    let missing = dir.join("nope.i");
    match Revlog::open_with_data(
        &missing,
        &dir.join("nope.d"),
        Some("filelog for x not found at y"),
        "d",
    ) {
        Err(crate::Error::Read(m)) => assert_eq!(m, "filelog for x not found at y"),
        other => panic!(
            "expected the not-found message, got {:?}",
            other.map(|_| ())
        ),
    }
    // Any other I/O error keeps its own text: an unreadable index is not reported as "not found".
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let unreadable = dir.join("locked.i");
        std::fs::write(&unreadable, [0u8; 64]).unwrap();
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&unreadable).is_err() {
            match Revlog::open_with_data(
                &unreadable,
                &dir.join("locked.d"),
                Some("filelog for x not found at y"),
                "d",
            ) {
                Err(crate::Error::Read(m)) => {
                    assert!(
                        !m.contains("not found at y"),
                        "a permission error is not 'not found': {m}"
                    );
                    assert!(m.contains("locked.i"), "{m}");
                }
                other => panic!("expected a Read error, got {:?}", other.map(|_| ())),
            }
        } else {
            eprintln!("skipping the permission half: this user can read a mode-000 file (root)");
        }
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let _ = std::fs::remove_dir_all(&dir);
}
