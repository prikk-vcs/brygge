//! Resource-ceiling tests for loading a dumpstream (RFC 010, CR-10). Small injected [`Limits`], never
//! gigabytes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::*;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_temp_path(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "brygge-svn-{label}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

#[test]
fn an_over_limit_dumpfile_is_refused_without_being_read() {
    let path = unique_temp_path("sparse-dump");
    // A sparse file: its declared length is huge, but no data blocks are allocated, so if brygge ever
    // actually read it fully this test would exhaust memory/time — it must not.
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(10 * 1024 * 1024 * 1024).unwrap(); // 10 GiB, sparse
    drop(file);

    let limits = Limits {
        max_dump_bytes: 1024,
        max_node_text_bytes: 1024,
        max_stderr_bytes: 1024,
        max_svnadmin_version_bytes: 4096,
    };
    let start = Instant::now();
    let result = Source::DumpFile(path.clone()).load_with(&limits);
    let elapsed = start.elapsed();
    let _ = std::fs::remove_file(&path);

    assert!(
        matches!(result, Err(crate::Error::ResourceLimit { .. })),
        "expected a resource-limit refusal, got {result:?}"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "refusal took {elapsed:?} — the metadata check should reject a 10 GiB sparse file instantly, \
         proving the file was never read"
    );
}

fn svnadmin_available() -> bool {
    Command::new("svnadmin")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn an_over_limit_svnadmin_dump_is_refused() {
    if !svnadmin_available() {
        eprintln!("skipping: svnadmin not available");
        return;
    }
    let repo = unique_temp_path("repo");
    assert!(
        Command::new("svnadmin")
            .arg("create")
            .arg(&repo)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    );
    // A big enough commit that svnadmin dump's stdout exceeds a tiny injected limit.
    let wc = unique_temp_path("wc");
    let repo_url = format!("file://{}", repo.display());
    assert!(
        Command::new("svn")
            .args(["checkout", "-q", &repo_url])
            .arg(&wc)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    );
    std::fs::write(wc.join("big.txt"), vec![b'x'; 8192]).unwrap();
    assert!(
        Command::new("svn")
            .args(["add", "-q", "big.txt"])
            .current_dir(&wc)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    );
    assert!(
        Command::new("svn")
            .args(["commit", "-q", "-m", "big file"])
            .current_dir(&wc)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    );
    let _ = std::fs::remove_dir_all(&wc);

    let limits = Limits {
        max_dump_bytes: 128,
        max_node_text_bytes: 1024,
        max_stderr_bytes: 1024,
        max_svnadmin_version_bytes: 4096,
    };
    let result = Source::LocalRepo(repo.clone()).load_with(&limits);
    let _ = std::fs::remove_dir_all(&repo);

    assert!(
        matches!(result, Err(crate::Error::ResourceLimit { .. })),
        "expected a resource-limit refusal, got {result:?}"
    );
}

// Drives a POSIX `sh` pipeline (`yes | head`) as the child process, so it is Unix-only.
#[cfg(unix)]
#[test]
fn a_child_writing_a_lot_of_stderr_does_not_deadlock() {
    if Command::new("sh").arg("-c").arg("true").status().is_err() {
        eprintln!("skipping: sh not available");
        return;
    }
    // Writes far more than the injected stderr cap (and past a typical OS pipe buffer) to stderr
    // *before* writing anything to stdout: a naive "read stdout to completion, then read stderr"
    // implementation would deadlock right here (the child blocks on a full stderr pipe with nobody
    // draining it, while brygge is still blocked reading stdout, which the child hasn't started
    // writing).
    let script = "(yes e 2>/dev/null | head -c 200000) 1>&2; (yes o 2>/dev/null | head -c 200000)";
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(script);
    let limits = Limits {
        max_dump_bytes: 4096,
        max_node_text_bytes: 1024,
        max_stderr_bytes: 4096,
        max_svnadmin_version_bytes: 4096,
    };

    let start = Instant::now();
    let result = run_and_capture(cmd, "stress test", "the dumpstream", &limits);
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(10),
        "run_and_capture took {elapsed:?} — a deadlock would hang indefinitely"
    );
    // 200,000 bytes of stdout against a 4096-byte cap: refused, not silently truncated-and-accepted.
    assert!(
        matches!(result, Err(crate::Error::ResourceLimit { .. })),
        "expected a resource-limit refusal, got {result:?}"
    );
}

// Drives a POSIX `sh` pipeline (`yes | head`) as the child process, so it is Unix-only.
#[cfg(unix)]
#[test]
fn an_over_limit_svnadmin_version_output_is_named_as_such_not_as_the_dumpstream() {
    // Review 010 F-3: the refusal names what overflowed.
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg("yes v 2>/dev/null | head -c 100000");
    let limits = Limits {
        max_dump_bytes: 64,
        max_node_text_bytes: 1024,
        max_stderr_bytes: 64,
        max_svnadmin_version_bytes: 64,
    };
    let result = run_and_capture(cmd, "version", "the svnadmin --version output", &limits);
    match result {
        Err(crate::Error::ResourceLimit { what, .. }) => {
            assert_eq!(what, "the svnadmin --version output");
        }
        other => panic!("expected a resource-limit refusal, got {other:?}"),
    }
}

#[test]
fn a_version_line_with_nothing_printable_neutralizes_to_empty() {
    // Review 010 F-2: an empty neutralized version is `Error::Open`, never `""`.
    assert!(matches!(
        version_from_output(b"\x01\x02\xff\t\n"),
        Err(crate::Error::Open(_))
    ));
    assert_eq!(version_from_output(b"1.14.5\nmore\n").unwrap(), "1.14.5");
    assert_eq!(neutralize_version(b"\x01\x02\xff\t"), "");
    assert_eq!(neutralize_version(b"1.14.5\x1b[2J"), "1.14.5[2J");
}

// Drives a POSIX `sh` pipeline (`yes | head`) as the child process, so it is Unix-only.
#[cfg(unix)]
#[test]
fn a_large_stderr_with_valid_stdout_under_the_cap_succeeds() {
    if Command::new("sh").arg("-c").arg("true").status().is_err() {
        eprintln!("skipping: sh not available");
        return;
    }
    // ~1 MiB of stderr, far past the 4 KiB cap, written before a small stdout payload. Before R-1
    // (2026-09-23 review 005), dropping the stderr read end once the cap was reached would SIGPIPE the
    // child partway through this write and the whole dump would fail; keeping the pipe open and draining
    // (discarding) past the cap lets the child finish normally.
    let script = "(yes e 2>/dev/null | head -c 1048576) 1>&2; printf ok";
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(script);
    let limits = Limits {
        max_dump_bytes: 4096,
        max_node_text_bytes: 1024,
        max_stderr_bytes: 4096,
        max_svnadmin_version_bytes: 4096,
    };

    let result = run_and_capture(cmd, "large stderr test", "the dumpstream", &limits);
    assert_eq!(
        result.unwrap(),
        b"ok",
        "the child must exit 0 and stdout must be returned whole, not killed by SIGPIPE on stderr"
    );
}

// ---- a source path that does not exist (0.3.0 cut prep) --------------------------------------------------

fn expect_open(result: Result<Vec<u8>, Error>) -> String {
    match result {
        Err(Error::Open(m)) => m,
        other => panic!("expected an open error, got {other:?}"),
    }
}

/// A missing dumpfile or repository path is `source not found: <path>` (an open error), and nothing is run:
/// the message is ours, not `svnadmin`'s ("Can't open file '<path>/format'").
#[test]
fn a_missing_path_is_source_not_found_for_a_dumpfile_and_for_a_repository() {
    let path = unique_temp_path("missing");
    let want = format!("source not found: {}", path.display());
    let limits = Limits::default();
    assert_eq!(
        expect_open(Source::DumpFile(path.clone()).load_with(&limits)),
        want
    );
    assert_eq!(
        expect_open(Source::LocalRepo(path).load_with(&limits)),
        want
    );
}

/// The remote refusal keeps precedence: a URL does not exist as a path either, and is still `remote-source`.
#[test]
fn a_url_is_still_the_remote_source_refusal_with_its_remedy() {
    let result = Source::LocalRepo(std::path::PathBuf::from("svn://example.org/r"))
        .load_with(&Limits::default());
    match result {
        Err(Error::FloorRefusal { feature, reason }) => {
            assert_eq!(feature, floor::REMOTE_SOURCE);
            assert!(
                reason.contains("svnrdump dump <url> > repo.dump"),
                "{reason}"
            );
        }
        other => panic!("expected the remote-source refusal, got {other:?}"),
    }
}

/// A FIFO is neither a dumpfile nor a repository directory: refused before anything reads it, so it does not
/// block (a reader of a FIFO with no writer would wait forever).
#[cfg(unix)]
#[test]
fn a_fifo_is_neither_a_dumpfile_nor_a_repository_and_does_not_block() {
    let path = unique_temp_path("fifo");
    if !Command::new("mkfifo")
        .arg(&path)
        .status()
        .is_ok_and(|s| s.success())
    {
        eprintln!("skipping: mkfifo is not available");
        return;
    }
    let want = format!(
        "SVN source is neither a dumpfile nor a repository directory: {}",
        path.display()
    );
    let (tx, rx) = std::sync::mpsc::channel();
    let p = path.clone();
    std::thread::spawn(move || {
        let limits = Limits::default();
        let a = Source::DumpFile(p.clone()).load_with(&limits);
        let b = Source::LocalRepo(p).load_with(&limits);
        let _ = tx.send((a, b));
    });
    let (dump, repo) = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("refused without blocking on the FIFO");
    let _ = std::fs::remove_file(&path);
    assert_eq!(expect_open(dump), want);
    assert_eq!(expect_open(repo), want);
}

/// A link to a directory or a file is what it points at (not refused as "neither").
#[cfg(unix)]
#[test]
fn a_symlink_to_a_file_or_directory_is_not_refused_as_neither() {
    let dir = unique_temp_path("linkdir");
    std::fs::create_dir(&dir).unwrap();
    let link = unique_temp_path("link");
    std::os::unix::fs::symlink(&dir, &link).unwrap();
    assert!(check_present(&link).is_ok());
    let _ = std::fs::remove_file(&link);
    let _ = std::fs::remove_dir(&dir);
}
