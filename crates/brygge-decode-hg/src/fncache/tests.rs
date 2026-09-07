//! Tests for fncache store-path encoding (RFC 005). The exact-string cases are ground truth taken from
//! real `hg` stores; the sweep re-checks against a live store when `hg` is present.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::process::Command;

use super::store_path;

#[test]
fn encodes_the_studied_cases_exactly() {
    assert_eq!(store_path("a.txt").unwrap(), "data/a.txt.i");
    assert_eq!(store_path("nor-mal.txt").unwrap(), "data/nor-mal.txt.i");
    assert_eq!(
        store_path("Sub/File.TXT").unwrap(),
        "data/_sub/_file._t_x_t.i"
    );
    assert_eq!(store_path("_under.txt").unwrap(), "data/__under.txt.i");
    assert_eq!(store_path(".hidden").unwrap(), "data/~2ehidden.i");
    assert_eq!(
        store_path(".config/App.ini").unwrap(),
        "data/~2econfig/_app.ini.i"
    );
}

#[test]
fn rejects_bad_paths_and_overlong_paths() {
    assert!(store_path("").is_err());
    assert!(store_path("/abs").is_err());
    assert!(store_path("a//b").is_err());
    assert!(
        store_path(&"x/".repeat(80)).is_err(),
        "overlong path is refused, not misread"
    );
}

fn hg_available() -> bool {
    Command::new("hg")
        .arg("--version")
        .env("HGRCPATH", "/dev/null")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn every_fncache_path_maps_to_an_existing_store_file() {
    if !hg_available() {
        eprintln!("skipping: hg not on PATH");
        return;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("brygge-hg-fn-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    struct Clean(PathBuf);
    impl Drop for Clean {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _c = Clean(dir.clone());
    let hg = |args: &[&str]| {
        let out = Command::new("hg")
            .env("HGRCPATH", "/dev/null")
            .arg("--cwd")
            .arg(&dir)
            .arg("-R")
            .arg(&dir)
            .arg("--config")
            .arg("ui.username=A <a@e.com>")
            .args(args)
            .output()
            .expect("hg");
        assert!(
            out.status.success(),
            "hg {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    hg(&["init", dir.to_str().unwrap()]);
    for name in [
        "a.txt",
        "Sub/File.TXT",
        "_under.txt",
        ".hidden",
        ".config/App.ini",
        "MixedCase.md",
    ] {
        let p = dir.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"x\n").unwrap();
    }
    hg(&["add"]);
    hg(&["commit", "-d", "1 0", "-m", "c0"]);
    let fncache = std::fs::read_to_string(dir.join(".hg/store/fncache")).unwrap();
    for line in fncache.lines().filter(|l| !l.trim().is_empty()) {
        // fncache entries look like "data/<logical>.i"
        let logical = line
            .strip_prefix("data/")
            .and_then(|s| s.strip_suffix(".i"))
            .unwrap_or_else(|| panic!("unexpected fncache entry {line:?}"));
        let encoded = store_path(logical).expect("encode fncache path");
        assert!(
            dir.join(".hg/store").join(&encoded).exists(),
            "logical {logical:?} -> {encoded:?} does not exist on disk"
        );
    }
}
