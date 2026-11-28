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
use std::io::Write as _;
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
    svn_dump_fixture_sized(FIXTURE_FILE_COUNT, FIXTURE_FILE_SIZE)
}

/// As [`svn_dump_fixture`], but with `file_count` files of `file_size` bytes each instead of the
/// handoff's own 40×4 MiB — used by the W1 cases below, which (unlike `decode`'s own cases) need no
/// large, slow write of their own to poll for reliably (W1's stderr collection polls a growing buffer,
/// never end of file, so there is no race to guard against by making the write itself slow): a much
/// smaller fixture keeps their own `decode`/`verify`/`encode git` setup steps fast under CI contention,
/// while still giving `encode git`'s partial repository real, non-empty content to work with.
/// A lighter fixture for `encode git`'s own W1 stop case: 10 files of 1 MiB each (10 MiB total,
/// one sixteenth of the handoff's own 40×4 MiB), which that case does not need in full — its own setup
/// already pays for a `decode svn` and an internal re-verify of the artifact *before* `encode git`'s
/// partial directory even exists, and W1's stderr collection polls a growing buffer rather than ever
/// waiting for end of file, so there is no race over a slow *write* left to guard against by keeping the
/// fixture huge. Still large enough that `git fast-import` cannot finish faster than this test can
/// observe the partial directory and send its stop.
fn encode_git_svn_dump_fixture() -> Vec<u8> {
    svn_dump_fixture_sized(10, 1024 * 1024)
}

fn svn_dump_fixture_sized(file_count: usize, file_size: usize) -> Vec<u8> {
    let mut buf = Vec::with_capacity(file_count * file_size + 4096);
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
        ("svn:log", "add files"),
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
    let mut content = vec![0u8; file_size];
    for i in 0..file_count {
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
/// captures `print!`/`eprintln!`/`eprint!`/`println!` by checking a thread-local `OUTPUT_CAPTURE`
/// override before any byte reaches the real descriptor — but that check lives in those macros alone.
/// Writing directly through `std::io::stderr()`'s own [`Write`] implementation never consults that
/// override at all, so it reaches the real file descriptor unconditionally, exactly as a child process's
/// own writes to its inherited stdio would, and with nothing spawned (review 210, carried from W0: the
/// previous form spawned `echo`/`cmd /C echo` per line — one needless process per line, and on Windows a
/// real shell interpreting the text, so a `&` or `|` in a path would have run as a command inside a
/// test).
fn log_line(text: &str) {
    let _ = writeln!(std::io::stderr().lock(), "{text}");
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

// ======================================================================================================
// RFC 022 batch W1: the watchdog extended to the three other writing commands, and D-4's leftover scan.
// ======================================================================================================

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// As `transform_integration.rs`'s own helper of the same name: every workspace binary built in the same
/// `cargo build`/`cargo test --workspace` session lands beside `brygge`'s own binary.
fn workspace_bin_dir() -> PathBuf {
    Path::new(bin())
        .parent()
        .expect("CARGO_BIN_EXE_brygge has a parent")
        .to_path_buf()
}

fn test_plugin_bin() -> PathBuf {
    let path = workspace_bin_dir().join(exe_name("brygge-transform-test-plugin"));
    assert!(
        path.is_file(),
        "brygge-transform-test-plugin was not built: build the workspace: `cargo test --workspace`"
    );
    path
}

fn install_transform_plugin(dir: &Path, name: &str, src: &Path) -> PathBuf {
    let dest = dir.join(exe_name(&format!("brygge-transform-{name}")));
    std::fs::copy(src, &dest).expect("copy plugin binary");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    dest
}

/// Run the real `brygge` binary (never a copy — these three cases do not need R-1's unique-copy
/// process-presence trick, which only `decode`'s own cases above rely on) to completion, asserting it
/// succeeds, and return its stdout/stderr pair.
fn run_to_completion(mut cmd: Command) -> std::process::Output {
    let out = cmd.output().expect("run brygge");
    assert!(
        out.status.success(),
        "setup step must succeed: {:?}\nstderr: {}",
        out,
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// Decodes `dump` (handoff §4's own 40×4 MiB pseudo-random fixture, reused here so the three new stop
/// cases below have the same reliably-slow-to-process content the existing `decode` cases do) into a
/// fresh IR artifact at `<dir>/big.ir`, via a real, run-to-completion `brygge decode svn`.
fn decode_big_artifact(dir: &Path, dump: &Path) -> PathBuf {
    let artifact = dir.join("big.ir");
    let mut cmd = Command::new(bin());
    // `--reconstruct-refs`: a plain `decode svn` with no refs at all would have every atom refused as
    // unreachable by `encode git`'s own preflight (RFC 019 D-7) — this fixture's `trunk` becomes a real
    // branch ref, so the one atom it carries is reachable. `verify --against-source`'s own re-decode
    // reads this same option back from the artifact's own provenance automatically, so this choice does
    // not need repeating at that call site.
    cmd.args(["decode", "svn", "--reconstruct-refs"])
        .arg(dump)
        .arg("--out")
        .arg(&artifact)
        .stdin(Stdio::null());
    run_to_completion(cmd);
    artifact
}

/// The first directory entry directly under `dir` whose name starts with `prefix`, polled for — so a
/// caller never has to predict an unpredictable suffix (a `nanos`-based attempt count) ahead of time.
fn poll_for_entry_with_prefix(dir: &Path, prefix: &str, within: Duration) -> Option<PathBuf> {
    let mut found = None;
    poll_until(within, || {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with(prefix) {
                found = Some(entry.path());
                return true;
            }
        }
        false
    });
    found
}

/// A recursive total-byte-length snapshot of everything under `dir` — used only to poll for "the
/// directory's content is visibly growing", never for correctness.
fn dir_total_bytes(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    total
}

/// Spawn `cmd` with piped stderr, returning the child, its pid, and a continuously updated shared
/// buffer of everything written to it so far.
///
/// **Deliberately never waits for the pipe to close (end of file) to read it** — `brygge transform`'s
/// own plugin, and `encode git`'s own `git init`/`git fast-import`, each inherit brygge's stdout/stderr
/// by design (`plugin.rs::run`'s own doc: "inheriting stdin/stdout/stderr", so the plugin's own output
/// reaches the user transparently; `GitEnv::apply`'s call sites leave Git's stdio at the Rust default,
/// which is also inherit). When brygge itself is killed mid-run, such a child can still be alive and
/// running on its own for a while afterward (the whole point of this suite's `--sleep-ms`/the large
/// fixture: there must *be* a window to kill brygge in) — and for as long as it is, it still holds that
/// same, shared pipe's write end open, so the pipe's real end-of-file can be arbitrarily delayed past
/// however long *that* unrelated child happens to keep running. A real terminal has no such problem (the
/// user simply sees the line the moment the watchdog writes it); this is purely an artifact of this
/// suite's own choice to collect output through a pipe, so this suite polls the growing buffer for the
/// content it expects instead of ever demanding the stream's own end.
struct RunningCommand {
    child: std::process::Child,
    pid: u32,
    stderr_buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
}

fn spawn_piped(mut cmd: Command) -> RunningCommand {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn brygge");
    let pid = child.id();
    let mut stderr = child.stderr.take().expect("piped stderr");
    let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let buf_writer = std::sync::Arc::clone(&buf);
    thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        loop {
            match stderr.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => buf_writer.lock().unwrap().extend_from_slice(&chunk[..n]),
            }
        }
    });
    RunningCommand {
        child,
        pid,
        stderr_buf: buf,
    }
}

/// The buffer's current content, as a lossy string — a snapshot, taken under the lock, never blocking on
/// anything further.
fn stderr_so_far(running: &RunningCommand) -> String {
    String::from_utf8_lossy(&running.stderr_buf.lock().unwrap()).into_owned()
}

/// Stop `running`, then poll its stderr buffer for `cond` to hold, for at most `within`; panics naming
/// what had actually arrived if `cond` never holds in time. Returns the buffer's content at the moment
/// `cond` first held (never the buffer's *final* content — nothing after that moment is this function's
/// concern; the caller's own assertions check exactly what it needs from what is returned).
fn stop_and_poll_stderr(
    mut running: RunningCommand,
    stop: impl FnOnce(u32),
    within: Duration,
    cond: impl Fn(&str) -> bool,
) -> String {
    stop(running.pid);
    let mut seen = String::new();
    let ok = poll_until(within, || {
        seen = stderr_so_far(&running);
        cond(&seen)
    });
    assert!(
        ok,
        "the expected stderr content did not appear within {within:?}; seen so far: {seen:?}"
    );
    let _ = running.child.wait();
    seen
}

/// As [`stop_and_poll_stderr`], for a case that expects a specific sentence to appear (`contains`,
/// never requiring the stream to end).
fn stop_and_collect_stderr_generic_within(
    running: RunningCommand,
    stop: impl FnOnce(u32),
    within: Duration,
    expect_contains: &str,
) -> String {
    stop_and_poll_stderr(running, stop, within, |s| s.contains(expect_contains))
}

fn stop_and_collect_stderr_generic(
    running: RunningCommand,
    stop: impl FnOnce(u32),
    expect_contains: &str,
) -> String {
    stop_and_collect_stderr_generic_within(running, stop, Duration::from_secs(10), expect_contains)
}

/// For a case that expects *no* sentence at all (`verify --against-source`, which registers nothing):
/// stop, wait a fixed, generous span for the watchdog to have exited on its own (nothing is registered,
/// so its own loop ends the moment the pipe closes, with nothing further to decide), then take one
/// snapshot. There is nothing to poll for here — the whole point is that nothing ever arrives.
fn stop_and_wait_then_snapshot(
    mut running: RunningCommand,
    stop: impl FnOnce(u32),
    wait: Duration,
) -> String {
    stop(running.pid);
    let _ = running.child.wait();
    thread::sleep(wait);
    stderr_so_far(&running)
}

#[cfg(unix)]
fn stop_unix(pid: u32) {
    send_signal(pid, "-INT");
}

#[cfg(windows)]
fn stop_windows(pid: u32) {
    taskkill(pid);
}

// ---- `encode git`, stopped while the partial repository exists (handoff §1, level 2) -------------------

fn run_encode_git_stopped_case(stop: impl FnOnce(u32)) {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let scratch = ScratchDir::new("stopped-encode-git");
    let dump = scratch.path().join("dump.svndump");
    std::fs::write(&dump, encode_git_svn_dump_fixture()).unwrap();
    let artifact = decode_big_artifact(scratch.path(), &dump);

    let out = scratch.path().join("repo.git");
    let out_name = out.file_name().unwrap().to_string_lossy().into_owned();
    let mut cmd = Command::new(bin());
    cmd.args(["encode", "git"])
        .arg(&artifact)
        .arg("--out")
        .arg(&out);
    let mut running = spawn_piped(cmd);
    let pid = running.pid;

    // The partial repository directory is created, and registered with the watchdog, before `git
    // init`/`fast-import` even start — poll for its name (an unpredictable `-<attempt>` suffix, though
    // in an empty scratch directory it is always `-0`) rather than assuming it.
    let partial_prefix = format!("{out_name}.partial-{pid}-");
    let partial =
        poll_for_entry_with_prefix(scratch.path(), &partial_prefix, Duration::from_secs(90));
    let Some(partial) = partial else {
        let exit_status = running.child.try_wait();
        let seen = stderr_so_far(&running);
        log_line(&format!(
            "stop_integration: encode git's partial directory never appeared; child try_wait()={exit_status:?}; stderr so far: {seen:?}"
        ));
        panic!("the partial repository directory never appeared; stderr so far: {seen}");
    };

    // R-5 (as the decode case above): poll for its content to be visibly growing, not merely for its
    // existence, so the kill lands while `git fast-import` is actually writing objects into it, never
    // in the narrow window right after `mkdir`/`git init` before any blob has been written.
    let grew = poll_until(Duration::from_secs(60), || dir_total_bytes(&partial) > 0);
    if !grew {
        let exit_status = running.child.try_wait();
        let seen = stderr_so_far(&running);
        panic!(
            "the partial repository {} never grew; the test caught nothing; child \
             try_wait()={exit_status:?}; stderr so far: {seen}",
            partial.display()
        );
    }

    // The watchdog's own D-3 ceiling for a directory is 30 s; this stop case can legitimately take up
    // to that long to report, so its own wait allows a comfortable margin past it.
    let stderr = stop_and_collect_stderr_generic_within(
        running,
        stop,
        Duration::from_secs(45),
        "interrupted:",
    );

    // D-3: the partial directory is removed once it stops changing (or, if a slow filesystem or a loaded
    // runner means it is still changing after the watchdog's own 30 s ceiling, it is named instead —
    // either way, never left as a bare, unregistered leak, and `<out>` itself never exists).
    let gone = poll_until(Duration::from_secs(35), || !partial.exists());
    assert!(!out.exists(), "no repository may exist after a clean stop");
    let report_path = {
        let mut p = out.as_os_str().to_os_string();
        p.push(".brygge-report");
        PathBuf::from(p)
    };
    assert!(
        !report_path.exists(),
        "no report may exist after a clean stop"
    );
    assert!(
        stderr.contains("interrupted:"),
        "the watchdog's sentence must appear: {stderr}"
    );
    if gone {
        // `display::human` (CR-19) doubles every literal backslash before a path reaches the
        // sentence — on Windows that means every path separator.
        let humanized = partial.display().to_string().replace('\\', "\\\\");
        assert!(
            stderr.contains(&humanized),
            "the sentence must name the removed partial directory: {stderr}"
        );
    } else {
        eprintln!(
            "note: the partial directory was still changing at the 30s ceiling on this run and was \
             named instead of removed — itself a pass of D-3's rule, not a failure: {stderr}"
        );
    }
}

#[cfg(unix)]
#[test]
fn encode_git_stopped_with_sigint_leaves_no_partial_repository() {
    run_encode_git_stopped_case(stop_unix);
}

#[cfg(windows)]
#[test]
fn encode_git_stopped_with_taskkill_leaves_no_partial_repository() {
    run_encode_git_stopped_case(stop_windows);
}

// ---- `transform`, stopped while its scratch output file exists (handoff §1, level 2) --------------------

fn run_transform_stopped_case(stop: impl FnOnce(u32)) {
    let scratch = ScratchDir::new("stopped-transform");
    // `transform` needs no source-control content at all (it reads an already-decoded artifact and
    // hands it to a plugin), so a tiny, directly-built artifact is enough — unlike the other two new
    // cases, nothing here depends on `decode`'s own slow fixture.
    let artifact = scratch.path().join("in.ir");
    std::fs::write(&artifact, empty_git_artifact()).unwrap();
    let plugin = test_plugin_bin();
    install_transform_plugin(scratch.path(), "slow", &plugin);

    let out = scratch.path().join("out.ir");
    let mut cmd = Command::new(bin());
    cmd.args(["transform", "slow"])
        .arg(&artifact)
        .arg("--out")
        .arg(&out)
        .arg("--")
        // Just long enough that the plugin is still sleeping (and so still holding brygge's inherited
        // stderr open, long after brygge itself has died — see `RunningCommand`'s own doc) well past
        // the handful of milliseconds this test needs to find the tmp file and send the stop; the
        // stderr collection below polls the growing buffer rather than ever waiting for that pipe to
        // close, so this no longer has to be long enough to matter for the test's own total runtime.
        .args(["--sleep-ms", "2000"])
        .env("PATH", scratch.path());
    let running = spawn_piped(cmd);
    let pid = running.pid;

    let tmp_prefix = format!(".brygge-transform-{pid}-");
    let tmp = poll_for_entry_with_prefix(scratch.path(), &tmp_prefix, Duration::from_secs(30))
        .expect("the scratch output file never appeared");

    let humanized = tmp.display().to_string().replace('\\', "\\\\");
    let expected_sentence = format!("interrupted: removed {humanized}; nothing was written");
    let stderr = stop_and_collect_stderr_generic(running, stop, &expected_sentence);

    assert!(
        poll_until(Duration::from_secs(10), || !tmp.exists()),
        "the temp path {} is still there 10 s after the stop",
        tmp.display()
    );
    assert!(!out.exists(), "no artifact may exist after a clean stop");
    assert!(
        stderr.trim_end().ends_with(&expected_sentence),
        "stderr does not end with the expected sentence.\nexpected suffix: {expected_sentence}\nactual: \
         {stderr}"
    );
}

#[cfg(unix)]
#[test]
fn transform_stopped_with_sigint_leaves_no_temp_artifact() {
    run_transform_stopped_case(stop_unix);
}

#[cfg(windows)]
#[test]
fn transform_stopped_with_taskkill_leaves_no_temp_artifact() {
    run_transform_stopped_case(stop_windows);
}

/// Builds a one-commit git repository in a throwaway directory, decodes it with a real `brygge decode
/// git`, and returns the resulting artifact's bytes. Skips by panicking with a clear message if `git` is
/// not on `PATH` — every CI runner this suite proves itself on has `git` (RFC 022 handoff §0), so this is
/// not an additional, optional dependency for this suite in practice.
fn empty_git_artifact() -> Vec<u8> {
    assert!(git_available(), "git must be on PATH for this test suite");
    let dir = empty_git_repo();
    let artifact = dir.0.join("a.ir");
    let out = Command::new(bin())
        .args(["decode", "git"])
        .arg(dir.path())
        .arg("--out")
        .arg(&artifact)
        .output()
        .expect("run brygge decode git");
    assert!(out.status.success(), "decode failed: {out:?}");
    std::fs::read(&artifact).unwrap()
}

fn empty_git_repo() -> ScratchDir {
    let dir = ScratchDir::new("minimal-git-src");
    let repo = dir.path();
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .current_dir(repo)
            .args(args)
            .env("GIT_AUTHOR_NAME", "A U Thor")
            .env("GIT_AUTHOR_EMAIL", "author@example.com")
            .env("GIT_AUTHOR_DATE", "2005-04-07T22:13:13 +0000")
            .env("GIT_COMMITTER_NAME", "C O Mitter")
            .env("GIT_COMMITTER_EMAIL", "committer@example.com")
            .env("GIT_COMMITTER_DATE", "2005-04-07T22:13:13 +0000")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?} failed");
    };
    git(&["init", "-q", "--initial-branch=main"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&["add", "-A"]);
    git(&["-c", "commit.gpgsign=false", "commit", "-q", "-m", "c1"]);
    dir
}

// ---- `verify --against-source`, stopped: nothing is ever registered, so the sentence never appears ----

fn run_verify_stopped_case(stop: impl FnOnce(u32)) {
    let scratch = ScratchDir::new("stopped-verify");
    let dump = scratch.path().join("dump.svndump");
    std::fs::write(&dump, svn_dump_fixture()).unwrap();
    let artifact = decode_big_artifact(scratch.path(), &dump);

    let mut cmd = Command::new(bin());
    cmd.args(["verify"])
        .arg(&artifact)
        .arg("--against-source")
        .arg(&dump);
    let mut running = spawn_piped(cmd);

    // Nothing `verify --against-source` ever does is registered with the watchdog (its own re-decode
    // spills through the same already-exempt content-store spill file `decode` itself uses, and it
    // writes no artifact of its own at all — see the request's evidence plan) — so there is no
    // registered path to poll for, the way every other case above does. The re-decode of the same
    // 40×4 MiB fixture is slow enough that a short, fixed wait reliably lands the stop while it is
    // still running; `try_wait` (never the process-name scan R-1 uses for a *uniquely named copy*,
    // which this case does not use) proves it had not already finished on its own, so the later
    // assertion that no sentence appears is not vacuous.
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        running.child.try_wait().unwrap(),
        None,
        "verify --against-source finished before the stop was even sent; this case would be vacuous"
    );

    let stderr = stop_and_wait_then_snapshot(running, stop, Duration::from_secs(2));

    assert!(
        !stderr.contains("interrupted:"),
        "verify --against-source registers nothing, so no watchdog sentence should ever appear: {stderr}"
    );
    log_line(&format!(
        "stop_integration: verify --against-source stopped; stderr was: {stderr:?}"
    ));
}

#[cfg(unix)]
#[test]
fn verify_against_source_stopped_with_sigint_prints_no_sentence() {
    run_verify_stopped_case(stop_unix);
}

#[cfg(windows)]
#[test]
fn verify_against_source_stopped_with_taskkill_prints_no_sentence() {
    run_verify_stopped_case(stop_windows);
}

// ---- D-4: a planted leftover from another pid is named, and left in place (handoff §1, level 2) --------

#[test]
fn a_planted_leftover_from_another_pid_is_named_and_left_in_place() {
    let scratch = ScratchDir::new("d4-leftover");
    let other_pid = std::process::id().wrapping_add(1).max(1);
    let leftover = scratch.path().join(format!(".x.ir.brygge-tmp-{other_pid}"));
    std::fs::write(&leftover, b"leftover content").unwrap();

    let artifact = scratch.path().join("in.ir");
    std::fs::write(&artifact, empty_git_artifact()).unwrap();
    let plugin = test_plugin_bin();
    install_transform_plugin(scratch.path(), "quick", &plugin);
    let out = scratch.path().join("out.ir");

    let output = Command::new(bin())
        .args(["transform", "quick"])
        .arg(&artifact)
        .arg("--out")
        .arg(&out)
        .env("PATH", scratch.path())
        .output()
        .expect("run brygge transform");
    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    // review 097 (RFC 022 batch W1): print D-4's own stderr lines so the request can quote exactly what
    // CI itself saw, even though this test passes (the assertions below check its exact content; this
    // is only so the raw text appears in the log for the review to cite directly).
    for line in stderr.lines() {
        log_line(&format!(
            "stop_integration: D-4 planted-leftover stderr line: {line}"
        ));
    }

    // `display::human` (CR-19) doubles every literal backslash before a path reaches the sentence —
    // on Windows that means every path separator (the same transform the decode cases above apply to
    // their own expected text).
    let humanized = leftover.display().to_string().replace('\\', "\\\\");
    let expected_prefix = format!(
        "found {humanized} ({}), a temporary file of brygge process {other_pid}; if no brygge is \
         running, remove it with: ",
        "leftover content".len()
    );
    assert!(
        stderr.contains(&expected_prefix),
        "D-4's exact notice must appear: {stderr}"
    );
    if cfg!(windows) {
        assert!(stderr.contains(&format!("del {humanized}")), "{stderr}");
    } else {
        assert!(stderr.contains(&format!("rm {humanized}")), "{stderr}");
    }
    // D-4 never deletes: the planted leftover is still exactly where it was.
    assert!(leftover.exists());
    assert_eq!(std::fs::read(&leftover).unwrap(), b"leftover content");
}
