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

/// Poll `cond` every `interval`, for at most `within`. Returns `true` the moment it holds, `false` if it
/// never does — the caller asserts on that, so the test never passes vacuously (handoff §4: "never
/// passes vacuously").
fn poll_every(within: Duration, interval: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    loop {
        if cond() {
            return true;
        }
        if start.elapsed() >= within {
            return false;
        }
        thread::sleep(interval);
    }
}

/// [`poll_every`] at the suite's ordinary 10 ms resolution, for conditions that are cheap to check
/// (a filesystem stat) and genuinely time-sensitive (catching the write phase in progress).
fn poll_until(within: Duration, cond: impl FnMut() -> bool) -> bool {
    poll_every(within, Duration::from_millis(10), cond)
}

fn tmp_artifact_path(dir: &Path, pid: u32) -> PathBuf {
    dir.join(format!(".a.ir.brygge-tmp-{pid}"))
}

/// R-1: a unique name for a throwaway copy of the real binary — for example `brygge-stoptest-4242-3`
/// (`.exe` appended on Windows) — specific enough that nothing else on a CI runner could coincidentally
/// share it. Since the watchdog is always `current_exe()` (`watchdog.rs`'s module doc), the watchdog
/// started from this copy carries this exact name too, which is the whole point.
fn unique_copy_name() -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let base = format!("brygge-stoptest-{}-{n}", std::process::id());
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base
    }
}

/// R-1: copy the real binary to a uniquely named file inside `dir` (a test's own [`ScratchDir`], so P-6's
/// `Drop` removes the copy along with everything else) and return its path. This touches only the test —
/// the production spawn path in `watchdog.rs` is untouched, and still always spawns `current_exe()`,
/// whatever that resolves to.
fn copy_binary_into(dir: &Path) -> PathBuf {
    let dest = dir.join(unique_copy_name());
    std::fs::copy(bin(), &dest).expect("copy the brygge binary into the test's scratch directory");
    // `std::fs::copy` already carries the source's permission bits on Unix (so the copy is already
    // executable); this is defensive, not load-bearing.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dest)
            .expect("stat the copied binary")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&dest, perms).expect("make the copied binary executable");
    }
    dest
}

/// Spawn `bin_path` against `dump`, writing to `<dir>/a.ir`, with its stderr piped so the caller can
/// read it after the process (and its watchdog, sharing the same pipe) have both closed it.
struct RunningDecode {
    child: std::process::Child,
    stderr_rx: mpsc::Receiver<Vec<u8>>,
    pid: u32,
    out: PathBuf,
}

fn start_decode(bin_path: impl AsRef<Path>, dump: &Path, dir: &Path) -> RunningDecode {
    let out = dir.join("a.ir");
    let mut child = Command::new(bin_path.as_ref())
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

/// Linux: no process anywhere on the system still carries this run's exact watchdog marker, checked by
/// reading every `/proc/<pid>/environ` (best effort: a process that exits mid-scan, or one this user
/// cannot read, is simply skipped, never treated as a match). Review 208 calls this "already stricter"
/// than the copied-binary check used on the other two platforms, so it is kept as is; `copy_name` is
/// unused here but kept in the shared signature below for a uniform call site.
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

/// R-1: is a process carrying this run's watchdog identity found right now? Linux keeps its own
/// marker-based check (review 208: already stricter); macOS and Windows query by the copied binary's
/// unique name, since the watchdog is always `current_exe()`, which is that exact copy.
#[cfg(target_os = "linux")]
fn watchdog_is_present(pid: u32, _copy_name: &str) -> bool {
    !no_process_carries_the_marker(&format!("BRYGGE_INTERNAL_WATCHDOG={pid}"))
}

/// R-1, macOS: `ps -A -o comm=` lists every process's command name/path; a line containing the copy's
/// unique name is treated as a match (deliberately not an exact-equals, since `comm` may render either
/// the bare name or a full path depending on the system — the name itself is unique enough either way).
#[cfg(target_os = "macos")]
fn watchdog_is_present(_pid: u32, copy_name: &str) -> bool {
    let output = Command::new("ps")
        .args(["-A", "-o", "comm="])
        .output()
        .expect("run ps -A -o comm=");
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().any(|line| line.contains(copy_name))
}

/// R-1, Windows: `tasklist /FI "IMAGENAME eq <name>.exe"` lists only processes with that exact image name
/// (already includes `.exe`, from [`unique_copy_name`]); any output line naming it is a match.
///
/// `/FO CSV` (never the default `TABLE` format, even with `/NH`): `TABLE` truncates the Image Name
/// column to a fixed display width (observed: a 26-character name like
/// `brygge-stoptest-3472-2.exe` was already cut off), so a line could fail to contain the full name even
/// while tasklist's own `/FI` filter had correctly matched and was displaying that exact process — a
/// false "not found" from display truncation, not from the process being absent. `CSV` writes every
/// field out in full, with no such limit.
#[cfg(windows)]
fn watchdog_is_present(_pid: u32, copy_name: &str) -> bool {
    let output = Command::new("tasklist")
        .args([
            "/FI",
            &format!("IMAGENAME eq {copy_name}"),
            "/FO",
            "CSV",
            "/NH",
        ])
        .output()
        .expect("run tasklist /FI ... /FO CSV /NH");
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().any(|line| line.contains(copy_name))
}

/// How often [`assert_watchdog_is_running`]/[`assert_no_watchdog_left`] re-check. `watchdog_is_present`
/// spawns a whole extra process (`ps -A` or `tasklist`, enumerating the system's entire process table) on
/// every call, unlike the suite's other, cheap (stat-based) polls — polling that at [`poll_until`]'s 10 ms
/// resolution measurably starved a concurrently running decode/watchdog of CPU on a loaded CI runner and
/// caused a real, reproducible failure (not a flake): the watchdog's own cleanup-and-exit was delayed
/// past the unrelated 10 s stderr-collection timeout in `stop_and_collect_stderr`. 200 ms still allows
/// comfortably more than enough attempts within the 10 s budget below for either outcome, which in the
/// un-starved case resolves within a few hundred milliseconds either way.
const WATCHDOG_PRESENCE_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Print `text` so it reaches the real CI log *even when the test passes*. `cargo test`'s harness
/// captures `print!`/`eprintln!` — and even a direct `std::io::stderr()` write made from *this*
/// process — and discards it for a test that does not fail, which would otherwise hide the exact
/// evidence review 208 asks for regardless of outcome. A *child* process's own writes to its inherited
/// stdio bypass that capture entirely: the harness can only redirect what this process's own API calls
/// target, never another process's real file descriptors. `echo` is a real executable on Unix (not only
/// a shell builtin), and `cmd /C echo` reaches the same real descriptor on Windows; every string this
/// is called with is built from plain alphanumerics, hyphens and `:`/`()`, so neither needs any shell
/// escaping.
fn log_line(text: &str) {
    #[cfg(unix)]
    {
        let _ = Command::new("echo").arg(text).status();
    }
    #[cfg(windows)]
    {
        let _ = Command::new("cmd").args(["/C", "echo", text]).status();
    }
}

/// R-1 point 4: prove the later absence check (`assert_no_watchdog_left`) is not vacuous — while the
/// decode (and the watchdog it started, the same copy under `current_exe()`) is still running, the exact
/// same query must find it.
fn assert_watchdog_is_running(pid: u32, copy_name: &str) {
    let found = poll_every(
        Duration::from_secs(10),
        WATCHDOG_PRESENCE_POLL_INTERVAL,
        || watchdog_is_present(pid, copy_name),
    );
    log_line(&format!(
        "stop_integration: while the decode (pid {pid}) was running, a process for {copy_name} was {}",
        if found {
            "found (expected)"
        } else {
            "NOT found"
        }
    ));
    assert!(
        found,
        "the watchdog ({copy_name}) was never observed running while the decode was in progress; the \
         later check for its absence would be vacuous"
    );
}

/// Within 10 s of the stop, no process anywhere on the system may carry this run's watchdog identity.
fn assert_no_watchdog_left(pid: u32, copy_name: &str) {
    let gone = poll_every(
        Duration::from_secs(10),
        WATCHDOG_PRESENCE_POLL_INTERVAL,
        || !watchdog_is_present(pid, copy_name),
    );
    log_line(&format!(
        "stop_integration: within 10s of the stop, a process for {copy_name} is now {}",
        if gone {
            "absent (expected)"
        } else {
            "STILL PRESENT"
        }
    ));
    assert!(
        gone,
        "a process for {copy_name} is still present 10 s after the stop"
    );
}

fn run_stopped_case(stop: impl FnOnce(u32)) {
    let scratch = ScratchDir::new("stopped");
    let dump = scratch.path().join("dump.svndump");
    std::fs::write(&dump, svn_dump_fixture()).unwrap();

    // R-1: a uniquely named copy of the real binary, inside this test's own scratch directory (so P-6's
    // `Drop` removes it with everything else). The watchdog is always `current_exe()`, so it carries
    // this exact unique name too.
    let copy = copy_binary_into(scratch.path());
    let copy_name = copy
        .file_name()
        .expect("the copy has a file name")
        .to_string_lossy()
        .into_owned();

    let running = start_decode(&copy, &dump, scratch.path());
    let pid = running.pid;
    let tmp = tmp_artifact_path(scratch.path(), pid);

    // Handoff §4 step 3: poll for the temp path, at most 60 s. Never passes vacuously — a timeout fails
    // the test outright, it never silently moves on.
    //
    // R-5: polling for mere existence would be flaky — `atomic_write` calls `guard.register(&tmp)`
    // right after `create_new(true)` creates the empty file, but *before* any content is written
    // (`commands.rs::atomic_write`: `create_new` -> `guard.register(&tmp)` -> `write_body` -> `rename`
    // -> `guard.release(&tmp)`). A kill landing in that narrow window would leave the file behind
    // (never registered, so the watchdog was never told to protect it), an avoidable flake rather than
    // a real bug. Polling for a nonzero length instead is safe proof that `write_body` — which only
    // starts after `register` has already completed — is under way.
    let appeared = poll_until(Duration::from_secs(60), || {
        std::fs::metadata(&tmp)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
    });
    assert!(
        appeared,
        "the write phase's temp path {} never appeared with content; the test caught nothing",
        tmp.display()
    );

    // R-1 point 4: prove the check below is not vacuous — while the decode is still running, the same
    // query that will shortly prove the watchdog's absence must find it present.
    assert_watchdog_is_running(pid, &copy_name);

    let out = running.out.clone();
    let stderr = stop_and_collect_stderr(running, stop);

    assert!(
        poll_until(Duration::from_secs(10), || !tmp.exists()),
        "the temp path {} is still there 10 s after the stop",
        tmp.display()
    );
    assert!(!out.exists(), "no artifact may exist after a clean stop");
    // `display::human` (CR-19) doubles every literal backslash before a path reaches the sentence, so
    // that an escape in the *output* is never ambiguous with one that arrived in the input — on Windows
    // that means every path separator. These fixture paths contain nothing else `human` would escape.
    let humanized = tmp.display().to_string().replace('\\', "\\\\");
    let expected_sentence = format!("interrupted: removed {humanized}; nothing was written");
    assert!(
        stderr.trim_end().ends_with(&expected_sentence),
        "stderr does not end with the expected sentence.\nexpected suffix: {expected_sentence}\nactual: \
         {stderr}"
    );
    assert_no_watchdog_left(pid, &copy_name);
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

    let running = start_decode(bin(), &dump, scratch.path());
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
