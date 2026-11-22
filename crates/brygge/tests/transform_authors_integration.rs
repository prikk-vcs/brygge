//! Level 2 (RFC 019 D-4 §0): decode a CVS and a Subversion fixture through the real CLI, run the
//! built-in `brygge transform authors`, then `brygge encode git` (which runs G2's check), and confirm
//! with real `git` that the mapped identities and `Brygge-Transformed` trailer are there.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_brygge")
}

fn scratch_dir(label: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "brygge-transform-authors-it-{label}-{}-{n}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn svnadmin_available() -> bool {
    Command::new("svnadmin")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(bin()).args(args).output().expect("run brygge")
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn cvs_single_rev_with_symbol(author: &str, date: &str, log: &str, content: &str) -> Vec<u8> {
    format!(
        "head\t1.1;\naccess;\nsymbols\n\tTAG1:1.1;\nlocks; strict;\n\n\n\
1.1\ndate\t{date};\tauthor {author};\tstate Exp;\nbranches;\nnext\t;\n\n\n\
desc\n@@\n\n\n\
1.1\nlog\n@{log}@\ntext\n@{content}@\n"
    )
    .into_bytes()
}

#[test]
fn a_cvs_authors_map_runs_through_inspect_transform_and_encode_git() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let repo_dir = scratch_dir("cvs-repo");
    std::fs::write(
        repo_dir.join("a.c,v"),
        cvs_single_rev_with_symbol("alice", "2024.01.01.12.00.00", "add a.c", "aaa\n"),
    )
    .unwrap();

    let work = scratch_dir("work");
    let decoded = work.join("decoded.ir");
    let out = run([
        "decode",
        "cvs",
        repo_dir.to_str().unwrap(),
        "--reconstruct-refs",
        "--out",
        decoded.to_str().unwrap(),
    ]
    .as_slice());
    assert!(out.status.success(), "{:?}", out);

    // `inspect --authors` lists the bare-name identity, already in map-file form.
    let listed = run(["inspect", "--authors", decoded.to_str().unwrap()].as_slice());
    assert!(listed.status.success(), "{:?}", listed);
    let listing = String::from_utf8_lossy(&listed.stdout);
    assert!(listing.contains("alice ="), "{listing}");
    assert!(listing.contains("author"), "{listing}");

    let map_path = work.join("authors.map");
    std::fs::write(
        &map_path,
        "# filled in by the test, as a user would after redirecting `inspect --authors`\n\
         alice = Alice Example <alice@example.invalid>\n",
    )
    .unwrap();

    let mapped = work.join("mapped.ir");
    let transformed = run([
        "transform",
        "authors",
        decoded.to_str().unwrap(),
        "--map",
        map_path.to_str().unwrap(),
        "--out",
        mapped.to_str().unwrap(),
        "--require-all",
    ]
    .as_slice());
    assert!(
        transformed.status.success(),
        "{}",
        String::from_utf8_lossy(&transformed.stderr)
    );
    let transform_stderr = String::from_utf8_lossy(&transformed.stderr);
    assert!(transform_stderr.contains("1 identit"), "{transform_stderr}");

    let repo = work.join("repo");
    let encoded = run([
        "encode",
        "git",
        mapped.to_str().unwrap(),
        "--out",
        repo.to_str().unwrap(),
    ]
    .as_slice());
    assert!(
        encoded.status.success(),
        "{}",
        String::from_utf8_lossy(&encoded.stderr)
    );

    let name = git_out(&repo, &["log", "-1", "--format=%an", "refs/tags/TAG1"]);
    let email = git_out(&repo, &["log", "-1", "--format=%ae", "refs/tags/TAG1"]);
    assert_eq!(name, "Alice Example");
    assert_eq!(email, "alice@example.invalid");

    let message = git_out(&repo, &["log", "-1", "--format=%B", "refs/tags/TAG1"]);
    assert!(
        message.contains("Brygge-Transformed: atom 0 authors"),
        "{message}"
    );
    assert!(message.contains("(author)"), "{message}");
    // No Brygge-Supplied for the field the map just filled in — review G1 F-1's own rule, which the
    // handoff's §4 asks this batch to prove still holds once an email now exists.
    assert!(!message.contains("Brygge-Supplied: author"), "{message}");
}

fn svn_props(pairs: &[(&str, &str)]) -> Vec<u8> {
    let mut buf = Vec::new();
    for (k, v) in pairs {
        buf.extend_from_slice(format!("K {}\n{k}\nV {}\n{v}\n", k.len(), v.len()).as_bytes());
    }
    buf.extend_from_slice(b"PROPS-END\n");
    buf
}

fn svn_dump_fixture() -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"SVN-fs-dump-format-version: 2\n\n");
    buf.extend_from_slice(b"UUID: 00000000-0000-0000-0000-000000000000\n\n");

    let r0_props = svn_props(&[]);
    buf.extend_from_slice(
        format!(
            "Revision-number: 0\nProp-content-length: {}\nContent-length: {}\n\n",
            r0_props.len(),
            r0_props.len()
        )
        .as_bytes(),
    );
    buf.extend_from_slice(&r0_props);
    buf.extend_from_slice(b"\n");

    for dir in ["trunk", "branches", "tags"] {
        let node = format!(
            "Node-path: {dir}\nNode-kind: dir\nNode-action: add\nProp-content-length: 10\n\
             Content-length: 10\n\nPROPS-END\n\n"
        );
        buf.extend_from_slice(node.as_bytes());
    }

    let r1_props = svn_props(&[
        ("svn:author", "bob"),
        ("svn:date", "2024-01-01T00:00:00.000000Z"),
        ("svn:log", "add trunk/f.txt"),
    ]);
    let content = b"bbb\n";
    buf.extend_from_slice(
        format!(
            "Revision-number: 1\nProp-content-length: {}\nContent-length: {}\n\n",
            r1_props.len(),
            r1_props.len()
        )
        .as_bytes(),
    );
    buf.extend_from_slice(&r1_props);
    buf.extend_from_slice(b"\n");
    buf.extend_from_slice(
        format!(
            "Node-path: trunk/f.txt\nNode-kind: file\nNode-action: add\n\
             Prop-content-length: 10\nText-content-length: {}\nContent-length: {}\n\n\
             PROPS-END\n",
            content.len(),
            10 + content.len()
        )
        .as_bytes(),
    );
    buf.extend_from_slice(content);
    buf.extend_from_slice(b"\n\n");

    buf
}

#[test]
fn an_svn_authors_map_runs_through_inspect_transform_and_encode_git() {
    if !git_available() || !svnadmin_available() {
        eprintln!("skipping: git or svnadmin not on PATH");
        return;
    }
    let work = scratch_dir("svn-work");
    let dump = work.join("dump.svndump");
    std::fs::write(&dump, svn_dump_fixture()).unwrap();

    let decoded = work.join("decoded.ir");
    let out = run([
        "decode",
        "svn",
        dump.to_str().unwrap(),
        "--reconstruct-refs",
        "--out",
        decoded.to_str().unwrap(),
    ]
    .as_slice());
    assert!(out.status.success(), "{:?}", out);

    let map_path = work.join("authors.map");
    std::fs::write(&map_path, "bob = Bob Example <bob@example.invalid>\n").unwrap();

    let mapped = work.join("mapped.ir");
    let transformed = run([
        "transform",
        "authors",
        decoded.to_str().unwrap(),
        "--map",
        map_path.to_str().unwrap(),
        "--out",
        mapped.to_str().unwrap(),
        "--require-all",
    ]
    .as_slice());
    assert!(
        transformed.status.success(),
        "{}",
        String::from_utf8_lossy(&transformed.stderr)
    );

    let repo = work.join("repo");
    let encoded = run([
        "encode",
        "git",
        mapped.to_str().unwrap(),
        "--out",
        repo.to_str().unwrap(),
    ]
    .as_slice());
    assert!(
        encoded.status.success(),
        "{}",
        String::from_utf8_lossy(&encoded.stderr)
    );

    let name = git_out(&repo, &["log", "-1", "--format=%an", "refs/heads/trunk"]);
    let email = git_out(&repo, &["log", "-1", "--format=%ae", "refs/heads/trunk"]);
    assert_eq!(name, "Bob Example");
    assert_eq!(email, "bob@example.invalid");

    let message = git_out(&repo, &["log", "-1", "--format=%B", "refs/heads/trunk"]);
    assert!(
        message.contains("Brygge-Transformed: atom 0 authors"),
        "{message}"
    );
}
