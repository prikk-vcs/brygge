use super::*;

// P-1(b): tested over `is_unmarked`'s explicit value, never by mutating the real process environment
// (`std::env::set_var`/`remove_var` are `unsafe` in this edition; the workspace forbids `unsafe`
// entirely).
#[test]
fn is_unmarked_is_true_only_when_the_marker_is_absent() {
    assert!(
        is_unmarked(None),
        "without the marker, a process must be allowed to start a watchdog"
    );
    assert!(
        !is_unmarked(Some("4242".into())),
        "with the marker already present (whatever its value), this process must never start \
         another watchdog — that is what bounds the recursion a fork bomb needs"
    );
}

// A sanity check that `may_spawn` itself reads the real environment correctly, in the one case every
// `cargo test` run is already in: unmarked (nothing in this test suite ever sets `MARKER_ENV`).
#[test]
fn may_spawn_is_true_in_an_ordinary_test_process() {
    assert!(may_spawn());
}

// P-1(c): the unit-test build's `Watchdog::start` cannot spawn anything — it refuses outright, every
// time, regardless of the environment.
#[test]
fn start_is_unsupported_in_a_unit_test_build() {
    match Watchdog::start() {
        Ok(_) => panic!("a unit-test build must never start a real watchdog"),
        Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::Unsupported),
    }
}
