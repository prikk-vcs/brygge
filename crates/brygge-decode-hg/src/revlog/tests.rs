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
    node.iter().map(|b| format!("{b:02x}")).collect()
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
