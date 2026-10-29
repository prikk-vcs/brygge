//! End-to-end subprocess proof of `brygge encode` (RFC 017 D-3, review request 059): the properties only
//! observable at the process boundary — real spawning, real `PATH` search, real exit codes, real signal
//! delivery — that a unit test inside `src/` cannot see.
//!
//! Two sibling binaries, built in the same `cargo build`/`cargo test --workspace` as `brygge` itself and
//! found alongside it in the target directory (never through `CARGO_BIN_EXE_*`, which Cargo sets only for
//! binaries of *this* package):
//! - `brygge-encode-test-plugin` (`tools/encode-test-plugin`): a controllable fake plugin for the lookup,
//!   verify-first, pass-through and argument tests.
//! - `brygge-encode-git-example` (`examples/encode-git`): the real reference encoder, used once as the
//!   actual plugin a user would run, to prove `encode` invokes it exactly as running it directly would.
//!
//! Both skip (with a "skipping: ... not built" message) if a `cargo test -p brygge` alone, without
//! `--workspace`, has not built them.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

const USAGE: i32 = 2;
const FAILURE: i32 = 1;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_brygge")
}

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The directory `brygge`'s own test binaries, and every other workspace binary built in the same
/// session, land in: `current_exe()`'s directory is `target/<profile>/deps/`; the binaries themselves
/// are one level up, in `target/<profile>/`.
fn workspace_bin_dir() -> PathBuf {
    let exe = std::env::current_exe().expect("current exe");
    let mut dir = exe.parent().expect("has a parent").to_path_buf();
    if dir.ends_with("deps") {
        dir.pop();
    }
    dir
}

fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// The path to a sibling workspace binary, or `None` if it has not been built (a bare `cargo test -p
/// brygge`, without `--workspace`, never builds it).
fn sibling_bin(name: &str) -> Option<PathBuf> {
    let path = workspace_bin_dir().join(exe_name(name));
    path.is_file().then_some(path)
}

fn test_plugin_bin() -> Option<PathBuf> {
    sibling_bin("brygge-encode-test-plugin")
}

fn git_example_bin() -> Option<PathBuf> {
    sibling_bin("brygge-encode-git-example")
}

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_dir(label: &str) -> TempDir {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "brygge-encode-itest-{label}-{}-{nanos}-{n}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    TempDir(dir)
}

/// Copies `src` into `dir` under the plugin name for `target` (`brygge-encode-<target>[.exe]`), with an
/// execute bit on Unix. Returns the copy's path.
fn install_plugin(dir: &Path, target: &str, src: &Path) -> PathBuf {
    let dest = dir.join(exe_name(&format!("brygge-encode-{target}")));
    std::fs::copy(src, &dest).expect("copy plugin binary");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    dest
}

fn write_non_executable_plugin(dir: &Path, target: &str) -> PathBuf {
    let dest = dir.join(exe_name(&format!("brygge-encode-{target}")));
    std::fs::write(&dest, b"not executable\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    dest
}

/// A valid, empty-but-decodable IR artifact: any path the `verify` checks on, when the test does not care
/// about its contents, built with `brygge decode` on a one-commit repository. Skips (returning `None`) if
/// `git` is not on `PATH`.
fn make_artifact(dir: &Path) -> Option<PathBuf> {
    if !git_available() {
        return None;
    }
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .current_dir(&repo)
            .args(args)
            .env("GIT_AUTHOR_NAME", "A U Thor")
            .env("GIT_AUTHOR_EMAIL", "author@example.com")
            .env("GIT_AUTHOR_DATE", "2005-04-07T22:13:13 +0000")
            .env("GIT_COMMITTER_NAME", "C O Mitter")
            .env("GIT_COMMITTER_EMAIL", "committer@example.com")
            .env("GIT_COMMITTER_DATE", "2005-04-07T22:13:13 +0000")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("run git");
        assert!(out.status.success(), "git {args:?} failed");
    };
    git(&["init", "-q", "--initial-branch=main"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&["add", "-A"]);
    git(&["-c", "commit.gpgsign=false", "commit", "-q", "-m", "c1"]);
    let artifact = dir.join("out.ir");
    let out = Command::new(bin())
        .args(["decode", "git"])
        .arg(&repo)
        .arg("--out")
        .arg(&artifact)
        .output()
        .expect("run brygge decode");
    assert!(out.status.success(), "decode failed: {out:?}");
    Some(artifact)
}

fn corrupt(artifact: &Path) {
    let mut bytes = std::fs::read(artifact).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    std::fs::write(artifact, bytes).unwrap();
}

// ---- lookup (proof item 2) --------------------------------------------------------------------------

#[test]
fn an_invalid_target_is_a_usage_error_before_any_search() {
    for bad in ["../x", "A", "x/y", ""] {
        let output = Command::new(bin())
            .args(["encode", bad, "out.ir"])
            .env("PATH", "")
            .output()
            .expect("run brygge encode");
        assert_eq!(output.status.code(), Some(USAGE), "target {bad:?}");
    }
    // 65 bytes: over the limit.
    let too_long = "a".repeat(65);
    let output = Command::new(bin())
        .args(["encode", &too_long, "out.ir"])
        .env("PATH", "")
        .output()
        .expect("run brygge encode");
    assert_eq!(output.status.code(), Some(USAGE));
}

#[test]
fn not_found_names_the_target_and_points_at_list() {
    let output = Command::new(bin())
        .args(["encode", "nonexistent-target", "out.ir"])
        .env("PATH", "")
        .output()
        .expect("run brygge encode");
    assert_eq!(output.status.code(), Some(USAGE));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no encoder for 'nonexistent-target'"),
        "{stderr}"
    );
    assert!(
        stderr.contains("brygge-encode-nonexistent-target"),
        "{stderr}"
    );
    assert!(stderr.contains("brygge encode --list"), "{stderr}");
}

#[test]
fn an_empty_and_a_relative_path_entry_are_skipped() {
    let Some(plugin) = test_plugin_bin() else {
        eprintln!(
            "skipping: brygge-encode-test-plugin not built (run `cargo build --workspace` first)"
        );
        return;
    };
    let scratch = temp_dir("relpath");
    // A decoy at the relative path, resolved against the child's current directory — must never be
    // found, since a relative PATH entry is always skipped.
    let decoy_dir = scratch.0.join("relative-dir");
    std::fs::create_dir_all(&decoy_dir).unwrap();
    install_plugin(&decoy_dir, "foo", &plugin);
    // The real plugin, at an absolute path later in PATH.
    let real_dir = scratch.0.join("real");
    std::fs::create_dir_all(&real_dir).unwrap();
    let real_plugin = install_plugin(&real_dir, "foo", &plugin);

    let sep = if cfg!(windows) { ';' } else { ':' };
    let path_value = format!("{sep}relative-dir{sep}{sep}{}", real_dir.display());
    let Some(artifact) = make_artifact(&scratch.0) else {
        eprintln!("skipping: git not on PATH");
        return;
    };
    let output = Command::new(bin())
        .args(["encode", "foo"])
        .arg(&artifact)
        .env("PATH", &path_value)
        .current_dir(&scratch.0)
        .output()
        .expect("run brygge encode");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains(&real_plugin.display().to_string()),
        "the banner must name the real (absolute-PATH) plugin: {stderr}"
    );
}

#[test]
fn a_non_executable_file_is_skipped_and_the_search_continues() {
    #[cfg(windows)]
    {
        eprintln!("skipping: this test's premise (a non-executable regular file) is Unix-only");
        return;
    }
    let Some(plugin) = test_plugin_bin() else {
        eprintln!(
            "skipping: brygge-encode-test-plugin not built (run `cargo build --workspace` first)"
        );
        return;
    };
    let scratch = temp_dir("nonexec");
    let a = scratch.0.join("a");
    std::fs::create_dir_all(&a).unwrap();
    write_non_executable_plugin(&a, "foo");
    let b = scratch.0.join("b");
    std::fs::create_dir_all(&b).unwrap();
    let real_plugin = install_plugin(&b, "foo", &plugin);

    let Some(artifact) = make_artifact(&scratch.0) else {
        eprintln!("skipping: git not on PATH");
        return;
    };
    let sep = if cfg!(windows) { ';' } else { ':' };
    let path_value = format!("{}{sep}{}", a.display(), b.display());
    let output = Command::new(bin())
        .args(["encode", "foo"])
        .arg(&artifact)
        .env("PATH", &path_value)
        .output()
        .expect("run brygge encode");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains(&real_plugin.display().to_string()),
        "{stderr}"
    );
}

#[test]
fn a_non_executable_file_alone_is_not_found() {
    #[cfg(windows)]
    {
        eprintln!("skipping: this test's premise (a non-executable regular file) is Unix-only");
        return;
    }
    let scratch = temp_dir("nonexec-alone");
    write_non_executable_plugin(&scratch.0, "foo");
    let output = Command::new(bin())
        .args(["encode", "foo", "out.ir"])
        .env("PATH", &scratch.0)
        .output()
        .expect("run brygge encode");
    assert_eq!(output.status.code(), Some(USAGE));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no encoder for 'foo'"), "{stderr}");
}

#[test]
fn two_directories_with_the_plugin_the_first_runs_and_list_shows_the_second_shadowed() {
    let Some(plugin) = test_plugin_bin() else {
        eprintln!(
            "skipping: brygge-encode-test-plugin not built (run `cargo build --workspace` first)"
        );
        return;
    };
    let scratch = temp_dir("shadow");
    let a = scratch.0.join("a");
    std::fs::create_dir_all(&a).unwrap();
    let b = scratch.0.join("b");
    std::fs::create_dir_all(&b).unwrap();
    let a_plugin = install_plugin(&a, "foo", &plugin);
    let b_plugin = install_plugin(&b, "foo", &plugin);
    let sep = if cfg!(windows) { ';' } else { ':' };
    let path_value = format!("{}{sep}{}", a.display(), b.display());

    let Some(artifact) = make_artifact(&scratch.0) else {
        eprintln!("skipping: git not on PATH");
        return;
    };
    let output = Command::new(bin())
        .args(["encode", "foo"])
        .arg(&artifact)
        .env("PATH", &path_value)
        .output()
        .expect("run brygge encode");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains(&a_plugin.display().to_string()),
        "the first PATH directory's plugin must be the one named/run: {stderr}"
    );

    let list_output = Command::new(bin())
        .args(["encode", "--list", "--format", "machine"])
        .env("PATH", &path_value)
        .output()
        .expect("run brygge encode --list");
    assert!(list_output.status.success());
    let stdout = String::from_utf8_lossy(&list_output.stdout);
    assert!(stdout.contains("encode_list_version=1"), "{stdout}");
    assert!(
        stdout.contains(&format!(
            "encoder.0.path={}",
            a_plugin.display().to_string().replace(':', "%3A")
        )) || stdout.contains(&format!("encoder.0.path={}", a_plugin.display())),
        "{stdout}"
    );
    assert!(stdout.contains("encoder.0.shadowed=false"), "{stdout}");
    assert!(stdout.contains("encoder.1.shadowed=true"), "{stdout}");
    let _ = b_plugin;
}

// ---- verify first (proof item 3) --------------------------------------------------------------------

#[test]
fn a_corrupted_artifact_is_refused_and_the_plugin_never_runs() {
    let Some(plugin) = test_plugin_bin() else {
        eprintln!(
            "skipping: brygge-encode-test-plugin not built (run `cargo build --workspace` first)"
        );
        return;
    };
    let scratch = temp_dir("verify-first");
    install_plugin(&scratch.0, "foo", &plugin);
    let Some(artifact) = make_artifact(&scratch.0) else {
        eprintln!("skipping: git not on PATH");
        return;
    };
    corrupt(&artifact);
    let marker = scratch.0.join("marker");
    let output = Command::new(bin())
        .args(["encode", "foo"])
        .arg(&artifact)
        .args(["--", "--marker"])
        .arg(&marker)
        .env("PATH", &scratch.0)
        .output()
        .expect("run brygge encode");
    assert!(!output.status.success());
    assert_ne!(output.status.code(), Some(0));
    assert!(!marker.exists(), "the plugin must never have started");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("verify_version") || stderr.contains("verify —"),
        "verify's own report must be printed: {stderr}"
    );
    assert!(output.stdout.is_empty(), "stdout must stay empty");
}

// ---- pass-through (proof item 4) and arguments (proof item 5) ----------------------------------------

#[test]
fn exit_codes_pass_through_unchanged() {
    let Some(plugin) = test_plugin_bin() else {
        eprintln!(
            "skipping: brygge-encode-test-plugin not built (run `cargo build --workspace` first)"
        );
        return;
    };
    let scratch = temp_dir("exit-codes");
    install_plugin(&scratch.0, "foo", &plugin);
    let Some(artifact) = make_artifact(&scratch.0) else {
        eprintln!("skipping: git not on PATH");
        return;
    };
    for code in [0, 3, 77] {
        let output = Command::new(bin())
            .args(["encode", "foo"])
            .arg(&artifact)
            .args(["--", "--then-exit", &code.to_string()])
            .env("PATH", &scratch.0)
            .output()
            .expect("run brygge encode");
        assert_eq!(output.status.code(), Some(code), "code {code}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(&format!("exited with {code}")), "{stderr}");
    }
}

#[cfg(unix)]
#[test]
fn a_self_killed_plugin_exits_1_with_the_signal_named() {
    let Some(plugin) = test_plugin_bin() else {
        eprintln!(
            "skipping: brygge-encode-test-plugin not built (run `cargo build --workspace` first)"
        );
        return;
    };
    let scratch = temp_dir("signal");
    install_plugin(&scratch.0, "foo", &plugin);
    let Some(artifact) = make_artifact(&scratch.0) else {
        eprintln!("skipping: git not on PATH");
        return;
    };
    let output = Command::new(bin())
        .args(["encode", "foo"])
        .arg(&artifact)
        .args(["--", "--then-abort"])
        .env("PATH", &scratch.0)
        .output()
        .expect("run brygge encode");
    assert_eq!(output.status.code(), Some(FAILURE));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("SIGABRT"), "{stderr}");
    assert!(stderr.contains('6'), "{stderr}");
}

#[test]
fn arguments_reach_the_plugin_verbatim_with_no_shell() {
    let Some(plugin) = test_plugin_bin() else {
        eprintln!(
            "skipping: brygge-encode-test-plugin not built (run `cargo build --workspace` first)"
        );
        return;
    };
    let scratch = temp_dir("argv");
    install_plugin(&scratch.0, "foo", &plugin);
    let Some(artifact) = make_artifact(&scratch.0) else {
        eprintln!("skipping: git not on PATH");
        return;
    };
    let tricky = [
        "has space",
        "has\"quote",
        "-leading-dash",
        "non-ascii-é-日本語",
        "$(echo shell-would-expand-this)",
        "a;b|c",
    ];
    let mut cmd = Command::new(bin());
    cmd.args(["encode", "foo"]).arg(&artifact).arg("--");
    for a in &tricky {
        cmd.arg(a);
    }
    cmd.env("PATH", &scratch.0);
    let output = cmd.output().expect("run brygge encode");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for a in &tricky {
        assert!(stdout.contains(&format!("argv: {a}")), "{stdout}");
    }
}

// ---- the example as a real plugin (proof item 1) ------------------------------------------------------

#[test]
fn the_git_example_runs_as_a_plugin_exactly_as_it_runs_directly() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let Some(example) = git_example_bin() else {
        eprintln!(
            "skipping: brygge-encode-git-example not built (run `cargo build --workspace` first)"
        );
        return;
    };
    let scratch = temp_dir("git-example-plugin");
    install_plugin(&scratch.0, "git-example", &example);
    let Some(artifact) = make_artifact(&scratch.0) else {
        eprintln!("skipping: git not on PATH");
        return;
    };

    let via_plugin = scratch.0.join("via-plugin.fi");
    let output = Command::new(bin())
        .args(["encode", "git-example"])
        .arg(&artifact)
        .args(["--", "--out"])
        .arg(&via_plugin)
        .env("PATH", &scratch.0)
        .output()
        .expect("run brygge encode git-example");
    assert!(output.status.success(), "{:?}", output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("encoding with the external plugin `brygge-encode-git-example`"),
        "{stderr}"
    );
    assert!(stderr.contains("not part of brygge"), "{stderr}");
    assert!(stderr.contains("exited with 0"), "{stderr}");

    let direct = scratch.0.join("direct.fi");
    let direct_output = Command::new(&example)
        .arg("--artifact")
        .arg(&artifact)
        .arg("--out")
        .arg(&direct)
        .output()
        .expect("run brygge-encode-git-example directly");
    assert!(direct_output.status.success());

    let via_plugin_bytes = std::fs::read(&via_plugin).unwrap();
    let direct_bytes = std::fs::read(&direct).unwrap();
    assert_eq!(
        via_plugin_bytes, direct_bytes,
        "running through `brygge encode` must produce the same fast-import stream as running the \
         example directly"
    );

    // And a real repository imports it without error.
    let target_repo = scratch.0.join("target-repo");
    std::fs::create_dir_all(&target_repo).unwrap();
    let init = Command::new("git")
        .current_dir(&target_repo)
        .args(["init", "-q", "--initial-branch=main"])
        .output()
        .expect("git init");
    assert!(init.status.success());
    let fast_import = Command::new("git")
        .current_dir(&target_repo)
        .arg("fast-import")
        .stdin(std::fs::File::open(&via_plugin).unwrap())
        .output()
        .expect("git fast-import");
    assert!(
        fast_import.status.success(),
        "{}",
        String::from_utf8_lossy(&fast_import.stderr)
    );
}
