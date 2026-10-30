//! An integration test, for one reason only: Cargo builds a package's own binaries for `cargo test` just
//! because it has integration tests, so this is what makes `cargo test --workspace` actually build
//! `brygge-transform-authors-example`'s binary — the one `crates/brygge`'s own proof-item-1 test
//! (RFC 018 D-6 §6) looks for alongside `brygge` (`CARGO_BIN_EXE_*` is set only for a package's own
//! binaries, used here, not for a sibling crate's).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::Command;

#[test]
fn help_names_the_three_required_flags() {
    let output = Command::new(env!("CARGO_BIN_EXE_brygge-transform-authors-example"))
        .arg("--help")
        .output()
        .expect("run brygge-transform-authors-example --help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--artifact"), "{stdout}");
    assert!(stdout.contains("--out"), "{stdout}");
    assert!(stdout.contains("--map"), "{stdout}");
    assert!(stdout.contains("unsupported example"), "{stdout}");
}

#[test]
fn missing_required_flags_is_a_usage_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_brygge-transform-authors-example"))
        .output()
        .expect("run brygge-transform-authors-example");
    assert_eq!(output.status.code(), Some(2));
}
