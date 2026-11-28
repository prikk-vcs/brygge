//! RFC 022 D-1/D-2/D-3, W0's gate (handoff `w0-platform-proof-handoff-v2.md` §4): a real `brygge decode`,
//! stopped part-way, leaves no temporary artifact and names it on stderr; a `decode` that is not stopped
//! is unaffected. Runs the real binary (`CARGO_BIN_EXE_brygge`), on all three systems.
//!
//! **This is spawning code under test, not a unit test.** It is proven in CI on `prove/w0-watchdog`
//! before it is ever run locally (HANDOFF §7a), and even then only through `tools/contained-test.sh`.
//!
//! The fixture is a hand-built SVN dumpstream (never `svnadmin`, so this test has no external
//! dependency), 40 files of 4 MiB of pseudo-random bytes each, so the artifact write visibly takes long
//! enough to poll for its temporary path reliably (handoff §4 step 1). It is written to the system
//! temporary directory for the duration of one test and removed afterward (P-6).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_brygge")
}

/// A fresh scratch directory for one test, removed on drop even if the test panics first (P-6: nothing
/// this suite creates in the system temporary directory is left behind).
struct ScratchDir(PathBuf);

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl ScratchDir {
    fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "brygge-stop-itest-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

/// A small, dependency-free, deterministic byte generator (xorshift64*): the point of "pseudo-random" is
/// non-degenerate content that does not compress to nothing, never cryptographic randomness, and never a
/// new dependency for a dev-only test fixture.
struct Xorshift64(u64);

impl Xorshift64 {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn fill(&mut self, buf: &mut [u8]) {
        let mut chunks = buf.chunks_exact_mut(8);
        for chunk in &mut chunks {
            chunk.copy_from_slice(&self.next_u64().to_le_bytes());
        }
        let rem = chunks.into_remainder();
        if !rem.is_empty() {
            let tail = self.next_u64().to_le_bytes();
            rem.copy_from_slice(&tail[..rem.len()]);
        }
    }
}

fn svn_props(pairs: &[(&str, &str)]) -> Vec<u8> {
    let mut buf = Vec::new();
    for (k, v) in pairs {
        buf.extend_from_slice(format!("K {}\n{k}\nV {}\n{v}\n", k.len(), v.len()).as_bytes());
    }
    buf.extend_from_slice(b"PROPS-END\n");
    buf
}

/// Handoff §4 step 1: 40 files of 4 MiB of pseudo-random bytes each, added in one revision under
/// `trunk/`, so the decoded artifact's own write visibly takes long enough to poll for reliably.
const FIXTURE_FILE_COUNT: usize = 40;
const FIXTURE_FILE_SIZE: usize = 4 * 1024 * 1024;

fn svn_dump_fixture() -> Vec<u8> {
    let mut buf = Vec::with_capacity(FIXTURE_FILE_COUNT * FIXTURE_FILE_SIZE + 4096);
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
        buf.extend_from_slice(
            format!(
                "Node-path: {dir}\nNode-kind: dir\nNode-action: add\nProp-content-length: 10\n\
                 Content-length: 10\n\nPROPS-END\n\n"
            )
            .as_bytes(),
        );
    }

    let r1_props = svn_props(&[
        ("svn:author", "stop-integration"),
        ("svn:date", "2024-01-01T00:00:00.000000Z"),
        ("svn:log", "add 40 large files"),
    ]);
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

    let mut rng = Xorshift64::new(0xB4C6_9E55);
    let mut content = vec![0u8; FIXTURE_FILE_SIZE];
    for i in 0..FIXTURE_FILE_COUNT {
        rng.fill(&mut content);
        buf.extend_from_slice(
            format!(
                "Node-path: trunk/f{i}.bin\nNode-kind: file\nNode-action: add\n\
                 Prop-content-length: 10\nText-content-length: {}\nContent-length: {}\n\n\
                 PROPS-END\n",
                content.len(),
                10 + content.len()
            )
            .as_bytes(),
        );
        buf.extend_from_slice(&content);
        buf.extend_from_slice(b"\n\n");
    }

    buf
}

/// The faithfulness statement `decode svn` prints first (`commands.rs::faithfulness_statement`), exactly
/// as the normal-case assertion below needs to reproduce it byte for byte.
const SVN_FAITHFULNESS_STATEMENT: &str = "Subversion: revisions are carried as recorded. Branches and \
tags are only a directory convention; with --reconstruct-refs they are reconstructed and marked derived. \
Authorship is Unverifiable.";

/// Poll `cond` every 10 ms, for at most `within`. Returns `true` the moment it holds, `false` if it never
/// does — the caller asserts on that, so the test never passes vacuously (handoff §4: "never passes
/// vacuously").
fn poll_until(within: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    loop {
        if cond() {
            return true;
        }
        if start.elapsed() >= within {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn tmp_artifact_path(dir: &Path, pid: u32) -> PathBuf {
    dir.join(format!(".a.ir.brygge-tmp-{pid}"))
}

/// Spawn the real binary against `dump`, writing to `<dir>/a.ir`, with its stderr piped so the caller can
/// read it after the process (and its watchdog, sharing the same pipe) have both closed it.
struct RunningDecode {
    child: std::process::Child,
    stderr_rx: mpsc::Receiver<Vec<u8>>,
    pid: u32,
    out: PathBuf,
}

fn start_decode(dump: &Path, dir: &Path) -> RunningDecode {
    let out = dir.join("a.ir");
    let mut child = Command::new(bin())
        .args(["decode", "svn"])
        .arg(dump)
        .arg("--out")
        .arg(&out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn brygge decode");
    let pid = child.id();
    let mut stderr = child.stderr.take().expect("piped stderr");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    RunningDecode {
        child,
        stderr_rx: rx,
        pid,
        out,
    }
}

/// Stop the child with a real OS command (never a library — handoff §4 step 4) and wait for the stderr
/// reader thread to see the pipe fully closed (both brygge and its watchdog, which inherits the same
/// stderr), within 10 s.
fn stop_and_collect_stderr(mut running: RunningDecode, stop: impl FnOnce(u32)) -> String {
    stop(running.pid);
    // Reap brygge's own exit so it is never left as a zombie; the watchdog is a separate process in its
    // own group and is not affected by this wait.
    let _ = running.child.wait();
    let bytes = running
        .stderr_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the watchdog's sentence did not reach stderr within 10 s");
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(unix)]
fn send_signal(pid: u32, signal: &str) {
    let status = Command::new("kill")
        .args([signal, &pid.to_string()])
        .status()
        .expect("run kill");
    assert!(status.success(), "kill {signal} {pid} failed");
}

#[cfg(windows)]
fn taskkill(pid: u32) {
    let status = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .status()
        .expect("run taskkill");
    assert!(status.success(), "taskkill /F /PID {pid} failed");
}

/// Linux-only, strong form of handoff §4 step 6: no process anywhere on the system still carries this
/// run's watchdog marker, checked by reading every `/proc/<pid>/environ` (best effort: a process that
/// exits mid-scan, or one this user cannot read, is simply skipped, never treated as a match).
#[cfg(target_os = "linux")]
fn no_process_carries_the_marker(marker: &str) -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return true; // cannot check; do not fail the test over an environment limitation.
    };
    for entry in entries.flatten() {
        let Ok(contents) = std::fs::read(entry.path().join("environ")) else {
            continue;
        };
        for var in contents.split(|&b| b == 0) {
            if var == marker.as_bytes() {
                return false;
            }
        }
    }
    true
}

#[cfg(target_os = "linux")]
fn assert_no_watchdog_left(pid: u32) {
    let marker = format!("BRYGGE_INTERNAL_WATCHDOG={pid}");
    assert!(
        poll_until(Duration::from_secs(10), || no_process_carries_the_marker(
            &marker
        )),
        "a process still carries {marker} 10 s after the stop"
    );
}

/// Elsewhere (macOS, Windows): the handoff's own mechanism for this check (§4 step 6, "a second,
/// test-only pipe it inherits") would require the production spawn path in `watchdog.rs` to grow a new,
/// test-only, environment-gated behavior — exactly the shape of change review 207 ruled out for *which*
/// program `start()` spawns, and which this implementation does not add unreviewed for *any* reason,
/// including this one. Flagged back to the architect in the batch's report rather than guessed at here;
/// this platform's proof stays on the temp-path and stderr-sentence assertions, which already require
/// the watchdog to have run its D-3 pass to completion.
#[cfg(not(target_os = "linux"))]
fn assert_no_watchdog_left(_pid: u32) {}

fn run_stopped_case(stop: impl FnOnce(u32)) {
    let scratch = ScratchDir::new("stopped");
    let dump = scratch.path().join("dump.svndump");
    std::fs::write(&dump, svn_dump_fixture()).unwrap();

    let running = start_decode(&dump, scratch.path());
    let pid = running.pid;
    let tmp = tmp_artifact_path(scratch.path(), pid);

    // Handoff §4 step 3: poll for the temp path, at most 60 s. Never passes vacuously — a timeout fails
    // the test outright, it never silently moves on.
    let appeared = poll_until(Duration::from_secs(60), || tmp.exists());
    assert!(
        appeared,
        "the write phase's temp path {} never appeared; the test caught nothing",
        tmp.display()
    );

    let out = running.out.clone();
    let stderr = stop_and_collect_stderr(running, stop);

    assert!(
        poll_until(Duration::from_secs(10), || !tmp.exists()),
        "the temp path {} is still there 10 s after the stop",
        tmp.display()
    );
    assert!(!out.exists(), "no artifact may exist after a clean stop");
    let expected_sentence = format!(
        "interrupted: removed {}; nothing was written",
        tmp.display()
    );
    assert!(
        stderr.trim_end().ends_with(&expected_sentence),
        "stderr does not end with the expected sentence.\nexpected suffix: {expected_sentence}\nactual: \
         {stderr}"
    );
    assert_no_watchdog_left(pid);
}

#[cfg(unix)]
#[test]
fn a_decode_stopped_with_sigint_leaves_no_temp_artifact() {
    run_stopped_case(|pid| send_signal(pid, "-INT"));
}

#[cfg(unix)]
#[test]
fn a_decode_stopped_with_sigkill_leaves_no_temp_artifact() {
    run_stopped_case(|pid| send_signal(pid, "-9"));
}

#[cfg(windows)]
#[test]
fn a_decode_stopped_with_taskkill_leaves_no_temp_artifact() {
    run_stopped_case(taskkill);
}

/// Handoff §4 "plus": a decode that is not stopped leaves no process and no temp path, and its stderr is
/// unaffected by the watchdog at all — byte-identical to what this exact codebase produced before this
/// batch (the faithfulness statement, then the one "wrote …" line, and nothing else; the watchdog, on a
/// normal end, "exits silently" per RFC 022 D-1, so there is nothing of its for this stderr to gain).
#[test]
fn a_decode_that_is_not_stopped_is_byte_identical_to_before_this_batch() {
    let scratch = ScratchDir::new("normal");
    let dump = scratch.path().join("dump.svndump");
    std::fs::write(&dump, svn_dump_fixture()).unwrap();

    let running = start_decode(&dump, scratch.path());
    let pid = running.pid;
    let out = running.out.clone();
    let tmp = tmp_artifact_path(scratch.path(), pid);
    let mut child = running.child;
    let status = child.wait().expect("wait for brygge decode");
    let bytes = running
        .stderr_rx
        .recv_timeout(Duration::from_secs(60))
        .expect("read brygge decode's stderr");
    let stderr = String::from_utf8_lossy(&bytes).into_owned();

    assert!(
        status.success(),
        "decode must succeed on the clean fixture: {stderr}"
    );
    assert!(out.exists(), "the artifact must exist on a normal end");
    assert!(!tmp.exists(), "no temp path survives a normal end");

    let artifact_len = std::fs::metadata(&out).unwrap().len();
    let expected = format!(
        "{SVN_FAITHFULNESS_STATEMENT}\nwrote {} ({artifact_len} bytes)\n",
        out.display()
    );
    assert_eq!(
        stderr, expected,
        "the watchdog must change nothing about the normal-path stderr"
    );
}
