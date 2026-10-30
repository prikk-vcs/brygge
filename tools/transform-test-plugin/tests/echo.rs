//! An integration test, for one reason only: Cargo builds a package's own binaries for `cargo test` just
//! because it has integration tests, so this is what makes `cargo test --workspace` actually build
//! `brygge-transform-test-plugin`'s binary. Without it, only this crate's unit-test harness gets built,
//! never the real binary `crates/brygge`'s own tests look for alongside `brygge` (`CARGO_BIN_EXE_*` is
//! set only for a package's own binaries, used here, not for a sibling crate's).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::Command;

#[test]
fn argv_is_echoed_verbatim() {
    let output = Command::new(env!("CARGO_BIN_EXE_brygge-transform-test-plugin"))
        .args(["--artifact", "irrelevant.ir", "has space", "-x"])
        .args(["--then-exit", "0"])
        .output()
        .expect("run brygge-transform-test-plugin");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("argv: --artifact"), "{stdout}");
    assert!(stdout.contains("argv: irrelevant.ir"), "{stdout}");
    assert!(stdout.contains("argv: has space"), "{stdout}");
    assert!(stdout.contains("argv: -x"), "{stdout}");
}

#[test]
fn marker_is_written_and_then_exit_controls_the_code() {
    let dir = std::env::temp_dir().join(format!(
        "brygge-transform-test-plugin-itest-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let marker = dir.join("marker");
    let output = Command::new(env!("CARGO_BIN_EXE_brygge-transform-test-plugin"))
        .arg("--marker")
        .arg(&marker)
        .args(["--then-exit", "3"])
        .output()
        .expect("run brygge-transform-test-plugin");
    assert_eq!(output.status.code(), Some(3));
    assert!(marker.exists());
    let _ = std::fs::remove_dir_all(&dir);
}
