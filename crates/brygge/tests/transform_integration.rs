//! End-to-end subprocess proof of `brygge transform` (RFC 018 D-6 §6, proof item 4): the properties only
//! observable at the process boundary — real spawning, real `PATH` search, real exit codes, real signal
//! delivery, a real temp file left behind or not — that a unit test inside `src/` cannot see.
//!
//! `brygge-transform-test-plugin` (`tools/transform-test-plugin`) is a sibling binary, built in the same
//! `cargo build`/`cargo test --workspace` as `brygge` itself, found alongside it in the target directory
//! (never through `CARGO_BIN_EXE_*`, which Cargo sets only for binaries of *this* package). If it is
//! missing, the tests below **fail**, naming the fix (`cargo test --workspace`), rather than silently
//! reporting `ok` for a test that never ran — the same rule `encode_integration.rs` follows.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

const USAGE: i32 = 2;
const VERIFY_FAILED: i32 = 50;
#[cfg(unix)]
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

fn sibling_bin(name: &str) -> PathBuf {
    let path = workspace_bin_dir().join(exe_name(name));
    assert!(
        path.is_file(),
        "{name} was not built: build the workspace: `cargo test --workspace`"
    );
    path
}

fn test_plugin_bin() -> PathBuf {
    sibling_bin("brygge-transform-test-plugin")
}

fn authors_example_bin() -> PathBuf {
    sibling_bin("brygge-transform-authors-example")
}

fn git_example_bin() -> PathBuf {
    sibling_bin("brygge-encode-git-example")
}

/// Copies `src` into `dir`, keeping its own file name (both real plugins here are already named
/// `brygge-<kind>-<name>[.exe]`, so unlike [`install_plugin`] there is nothing to rename).
fn install_as_is(dir: &Path, src: &Path) -> PathBuf {
    let dest = dir.join(src.file_name().expect("a binary path has a file name"));
    std::fs::copy(src, &dest).expect("copy plugin binary");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    dest
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
        "brygge-transform-itest-{label}-{}-{nanos}-{n}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    TempDir(dir)
}

fn install_plugin(dir: &Path, name: &str, src: &Path) -> PathBuf {
    let dest = dir.join(exe_name(&format!("brygge-transform-{name}")));
    std::fs::copy(src, &dest).expect("copy plugin binary");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    dest
}

/// A valid, decodable, one-commit IR artifact. Skips (returning `None`) if `git` is not on `PATH`.
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
    let artifact = dir.join("in.ir");
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

/// No `.brygge-transform-*` temp file was left behind in `dir`.
fn no_temp_file_left(dir: &Path) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        assert!(
            !name.starts_with(".brygge-transform-"),
            "a temp file was left behind: {name}"
        );
    }
}

// ---- lookup ------------------------------------------------------------------------------------------

#[test]
fn an_invalid_name_is_a_usage_error_before_any_search() {
    for bad in ["../x", "A", "x/y", ""] {
        let output = Command::new(bin())
            .args(["transform", bad, "in.ir", "--out", "out.ir"])
            .env("PATH", "")
            .output()
            .expect("run brygge transform");
        assert_eq!(output.status.code(), Some(USAGE), "name {bad:?}");
    }
}

#[test]
fn not_found_names_the_transform_and_points_at_list() {
    let scratch = temp_dir("notfound");
    let out = scratch.0.join("out.ir");
    let output = Command::new(bin())
        .args(["transform", "nonexistent-name", "in.ir", "--out"])
        .arg(&out)
        .env("PATH", "")
        .output()
        .expect("run brygge transform");
    assert_eq!(output.status.code(), Some(USAGE));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no transform for 'nonexistent-name'"),
        "{stderr}"
    );
    assert!(
        stderr.contains("brygge-transform-nonexistent-name"),
        "{stderr}"
    );
    assert!(stderr.contains("brygge transform --list"), "{stderr}");
}

#[test]
fn out_same_as_artifact_is_a_usage_error() {
    let scratch = temp_dir("same-file");
    let Some(artifact) = make_artifact(&scratch.0) else {
        eprintln!("skipping: git not on PATH");
        return;
    };
    let output = Command::new(bin())
        .args(["transform", "foo"])
        .arg(&artifact)
        .arg("--out")
        .arg(&artifact)
        .env("PATH", "")
        .output()
        .expect("run brygge transform");
    assert_eq!(output.status.code(), Some(USAGE));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--out must not be the same file"),
        "{stderr}"
    );
}

// ---- verify-first -------------------------------------------------------------------------------------

#[test]
fn a_corrupted_artifact_is_refused_and_the_plugin_never_runs() {
    let plugin = test_plugin_bin();
    let scratch = temp_dir("verify-first");
    install_plugin(&scratch.0, "foo", &plugin);
    let Some(artifact) = make_artifact(&scratch.0) else {
        eprintln!("skipping: git not on PATH");
        return;
    };
    corrupt(&artifact);
    let marker = scratch.0.join("marker");
    let out = scratch.0.join("out.ir");
    let output = Command::new(bin())
        .args(["transform", "foo"])
        .arg(&artifact)
        .arg("--out")
        .arg(&out)
        .args(["--", "--marker"])
        .arg(&marker)
        .env("PATH", &scratch.0)
        .output()
        .expect("run brygge transform");
    assert!(!output.status.success());
    assert_ne!(output.status.code(), Some(0));
    assert!(!marker.exists(), "the plugin must never have started");
    assert!(!out.exists(), "--out must stay untouched");
    no_temp_file_left(&scratch.0);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("verify_version") || stderr.contains("verify —"),
        "verify's own report must be printed: {stderr}"
    );
}

// ---- pass-through / arguments -------------------------------------------------------------------------

#[test]
fn exit_codes_pass_through_unchanged_and_leave_no_temp_file() {
    let plugin = test_plugin_bin();
    let scratch = temp_dir("exit-codes");
    install_plugin(&scratch.0, "foo", &plugin);
    let Some(artifact) = make_artifact(&scratch.0) else {
        eprintln!("skipping: git not on PATH");
        return;
    };
    for code in [3, 77] {
        let out = scratch.0.join(format!("out-{code}.ir"));
        let output = Command::new(bin())
            .args(["transform", "foo"])
            .arg(&artifact)
            .arg("--out")
            .arg(&out)
            .args(["--", "--then-exit", &code.to_string()])
            .env("PATH", &scratch.0)
            .output()
            .expect("run brygge transform");
        assert_eq!(output.status.code(), Some(code), "code {code}");
        assert!(!out.exists());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(&format!("exited with {code}")), "{stderr}");
    }
    no_temp_file_left(&scratch.0);
}

#[cfg(unix)]
#[test]
fn a_self_killed_plugin_exits_1_with_the_signal_named_and_leaves_no_temp_file() {
    let plugin = test_plugin_bin();
    let scratch = temp_dir("signal");
    install_plugin(&scratch.0, "foo", &plugin);
    let Some(artifact) = make_artifact(&scratch.0) else {
        eprintln!("skipping: git not on PATH");
        return;
    };
    let out = scratch.0.join("out.ir");
    let output = Command::new(bin())
        .args(["transform", "foo"])
        .arg(&artifact)
        .arg("--out")
        .arg(&out)
        .args(["--", "--then-abort"])
        .env("PATH", &scratch.0)
        .output()
        .expect("run brygge transform");
    assert_eq!(output.status.code(), Some(FAILURE));
    assert!(!out.exists());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("SIGABRT"), "{stderr}");
    no_temp_file_left(&scratch.0);
}

#[test]
fn arguments_reach_the_plugin_verbatim_with_no_shell() {
    let plugin = test_plugin_bin();
    let scratch = temp_dir("argv");
    install_plugin(&scratch.0, "foo", &plugin);
    let Some(artifact) = make_artifact(&scratch.0) else {
        eprintln!("skipping: git not on PATH");
        return;
    };
    let out = scratch.0.join("out.ir");
    let tricky = [
        "has space",
        "has\"quote",
        "-leading-dash",
        "non-ascii-é-日本語",
        "$(echo shell-would-expand-this)",
        "a;b|c",
    ];
    let mut cmd = Command::new(bin());
    cmd.args(["transform", "foo"])
        .arg(&artifact)
        .arg("--out")
        .arg(&out)
        .arg("--");
    for a in &tricky {
        cmd.arg(a);
    }
    cmd.env("PATH", &scratch.0);
    let output = cmd.output().expect("run brygge transform");
    assert!(output.status.success(), "{:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    for a in &tricky {
        assert!(stdout.contains(&format!("argv: {a}")), "{stdout}");
    }
}

// ---- an honest no-op transform, end to end ------------------------------------------------------------

#[test]
fn an_honest_no_op_transform_is_accepted_and_overwrites_out() {
    let plugin = test_plugin_bin();
    let scratch = temp_dir("honest");
    install_plugin(&scratch.0, "foo", &plugin);
    let Some(artifact) = make_artifact(&scratch.0) else {
        eprintln!("skipping: git not on PATH");
        return;
    };
    let out = scratch.0.join("out.ir");
    // `--out` already exists: `decode --out`'s own behavior (silently, atomically overwritten).
    std::fs::write(&out, b"stale").unwrap();
    let output = Command::new(bin())
        .args(["transform", "foo"])
        .arg(&artifact)
        .arg("--out")
        .arg(&out)
        .env("PATH", &scratch.0)
        .output()
        .expect("run brygge transform");
    assert!(output.status.success(), "{:?}", output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("transforming with the external plugin `brygge-transform-foo`"),
        "{stderr}"
    );
    assert!(stderr.contains("not part of brygge"), "{stderr}");
    assert!(stderr.contains("step 0"), "{stderr}");
    no_temp_file_left(&scratch.0);

    let verify_output = Command::new(bin())
        .args(["verify"])
        .arg(&out)
        .output()
        .expect("run brygge verify");
    assert!(verify_output.status.success(), "{:?}", verify_output);

    let inspect_output = Command::new(bin())
        .args(["inspect"])
        .arg(&out)
        .output()
        .expect("run brygge inspect");
    assert!(inspect_output.status.success());
    let stdout = String::from_utf8_lossy(&inspect_output.stdout);
    assert!(stdout.contains("transforms: 1"), "{stdout}");
}

#[test]
fn a_dishonest_output_is_refused_out_is_untouched_and_no_temp_file_remains() {
    let plugin = test_plugin_bin();
    let scratch = temp_dir("dishonest");
    install_plugin(&scratch.0, "foo", &plugin);
    let Some(artifact) = make_artifact(&scratch.0) else {
        eprintln!("skipping: git not on PATH");
        return;
    };
    let out = scratch.0.join("out.ir");
    let output = Command::new(bin())
        .args(["transform", "foo"])
        .arg(&artifact)
        .arg("--out")
        .arg(&out)
        .args(["--", "--mode", "dishonest-output"])
        .env("PATH", &scratch.0)
        .output()
        .expect("run brygge transform");
    assert_eq!(output.status.code(), Some(VERIFY_FAILED));
    assert!(!out.exists(), "--out must stay untouched");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("the transform's output was refused"),
        "{stderr}"
    );
    no_temp_file_left(&scratch.0);
}

// ---- the real example, as a plugin, through the whole chain (review 103 P-1) --------------------------

/// A decoded, one-commit IR artifact whose single commit's author/committer is `author`/`email`
/// exactly — unlike [`make_artifact`], which uses a fixed identity, this lets the test name the one
/// identity the map file will remap. Skips (returning `None`) if `git` is not on `PATH`.
fn make_artifact_with_author(dir: &Path, author: &str, email: &str) -> Option<PathBuf> {
    if !git_available() {
        return None;
    }
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .current_dir(&repo)
            .args(args)
            .env("GIT_AUTHOR_NAME", author)
            .env("GIT_AUTHOR_EMAIL", email)
            .env("GIT_AUTHOR_DATE", "2005-04-07T22:13:13 +0000")
            .env("GIT_COMMITTER_NAME", author)
            .env("GIT_COMMITTER_EMAIL", email)
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
    let artifact = dir.join("in.ir");
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

fn machine_value(lines: &str, key: &str) -> Option<String> {
    lines
        .lines()
        .find_map(|l| l.strip_prefix(key).map(ToString::to_string))
}

#[test]
fn the_real_example_runs_as_a_plugin_through_the_whole_chain() {
    let transform_plugin = authors_example_bin();
    let encode_plugin = git_example_bin();
    let scratch = temp_dir("real-example-chain");
    let path_dir = scratch.0.join("path");
    std::fs::create_dir_all(&path_dir).unwrap();
    install_as_is(&path_dir, &transform_plugin);
    install_as_is(&path_dir, &encode_plugin);

    let Some(artifact) = make_artifact_with_author(&scratch.0, "Old Name", "old@example.com")
    else {
        eprintln!("skipping: git not on PATH");
        return;
    };
    let repo = scratch.0.join("repo");

    let map = scratch.0.join("map.txt");
    std::fs::write(
        &map,
        "Old Name <old@example.com> = New Name <new@example.com>\n",
    )
    .unwrap();

    // 1. `brygge transform authors-example <artifact> --out <file> -- --map <map>`, the real example on
    //    a temporary PATH.
    let transformed = scratch.0.join("transformed.ir");
    let transform_output = Command::new(bin())
        .args(["transform", "authors-example"])
        .arg(&artifact)
        .arg("--out")
        .arg(&transformed)
        .args(["--", "--map"])
        .arg(&map)
        .env("PATH", &path_dir)
        .output()
        .expect("run brygge transform authors-example");
    assert!(transform_output.status.success(), "{:?}", transform_output);

    // 2a. `verify` passes.
    let verify = Command::new(bin())
        .arg("verify")
        .arg(&transformed)
        .output()
        .expect("run brygge verify");
    assert!(verify.status.success(), "{:?}", verify);

    // 2b. `verify --against-source` gives `root-corresponds` (the output has a one-step chain; only its
    // root — the input artifact's own digest — can ever correspond to a fresh decode of the source).
    let verify_source = Command::new(bin())
        .arg("verify")
        .arg(&transformed)
        .arg("--against-source")
        .arg(&repo)
        .args(["--format", "machine"])
        .output()
        .expect("run brygge verify --against-source");
    assert!(verify_source.status.success(), "{:?}", verify_source);
    let stdout = String::from_utf8_lossy(&verify_source.stdout);
    assert!(
        stdout.contains("verify.against_source=root-corresponds"),
        "{stdout}"
    );

    // 2c. `inspect --format machine` gives `transforms=1` and `transform.0.changed_atoms` > 0.
    let inspect = Command::new(bin())
        .arg("inspect")
        .arg(&transformed)
        .args(["--format", "machine"])
        .output()
        .expect("run brygge inspect");
    assert!(inspect.status.success(), "{:?}", inspect);
    let stdout = String::from_utf8_lossy(&inspect.stdout);
    assert!(stdout.contains("transforms=1"), "{stdout}");
    let changed_atoms: u64 = machine_value(&stdout, "transform.0.changed_atoms=")
        .unwrap_or_else(|| panic!("no transform.0.changed_atoms line: {stdout}"))
        .parse()
        .unwrap();
    assert!(changed_atoms > 0, "{stdout}");

    // 3. `brygge encode git-example` on the output gives `Brygge-Transformed:` trailers naming
    //    `author`, and `git fast-import` accepts it.
    let fast_import_stream = scratch.0.join("out.fi");
    let encode_output = Command::new(bin())
        .args(["encode", "git-example"])
        .arg(&transformed)
        .args(["--", "--out"])
        .arg(&fast_import_stream)
        .env("PATH", &path_dir)
        .output()
        .expect("run brygge encode git-example");
    assert!(encode_output.status.success(), "{:?}", encode_output);

    let fi_bytes = std::fs::read(&fast_import_stream).unwrap();
    let fi_text = String::from_utf8_lossy(&fi_bytes);
    let transformed_line = fi_text
        .lines()
        .find(|l| l.contains("Brygge-Transformed:"))
        .unwrap_or_else(|| panic!("no Brygge-Transformed: trailer: {fi_text}"));
    assert!(
        transformed_line.contains("author"),
        "the trailer must name author: {transformed_line}"
    );

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
        .stdin(std::fs::File::open(&fast_import_stream).unwrap())
        .output()
        .expect("git fast-import");
    assert!(
        fast_import.status.success(),
        "{}",
        String::from_utf8_lossy(&fast_import.stderr)
    );
}
