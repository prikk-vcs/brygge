//! Tests for the IR contract version gate (RFC 011 D-3).

use super::*;

#[test]
fn current_is_readable() {
    assert!(CURRENT.accepts());
    assert!(ensure_readable(CURRENT).is_ok());
}

#[test]
fn the_contract_is_recut_to_0_2_0() {
    // RFC 011 D-1(a)/D-3: the contract is re-cut off the 1.0.0 freeze while still major 0.
    assert_eq!(CURRENT, ContractVersion::new(0, 2, 0));
}

#[test]
fn a_non_critical_only_patch_bump_is_still_accepted() {
    // RFC 011 D-3: within the same 0.y, a higher patch (non-critical-only additions) is readable.
    let patched = ContractVersion::new(CURRENT.major, CURRENT.minor, CURRENT.patch + 9);
    assert!(patched.accepts());
    assert!(ensure_readable(patched).is_ok());
}

#[test]
fn a_different_minor_under_major_zero_is_refused() {
    // RFC 011 D-3: while major 0, a reader accepts only its own exact 0.y — not an older or newer y.
    let older_minor = ContractVersion::new(0, CURRENT.minor - 1, 0);
    assert!(!older_minor.accepts());
    let newer_minor = ContractVersion::new(0, CURRENT.minor + 1, 0);
    assert!(!newer_minor.accepts());
    match ensure_readable(newer_minor) {
        Err(crate::Error::UnsupportedContract { found, supported }) => {
            assert_eq!(found, newer_minor);
            assert_eq!(supported, CURRENT);
        }
        other => panic!("expected UnsupportedContract, got {other:?}"),
    }
}

#[test]
fn a_different_major_is_refused() {
    let newer_major = ContractVersion::new(CURRENT.major + 1, 0, 0);
    assert!(!newer_major.accepts());
    assert!(ensure_readable(newer_major).is_err());
}
