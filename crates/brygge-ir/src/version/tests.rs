//! Tests for the IR contract version gate (RFC 011 D-3).

use super::*;

#[test]
fn current_is_readable() {
    assert!(CURRENT.accepts());
    assert!(ensure_readable(CURRENT).is_ok());
}

#[test]
fn the_contract_is_0_3_0() {
    // RFC 018 D-1: compact artifacts re-cut the contract to 0.3.0.
    assert_eq!(CURRENT, ContractVersion::new(0, 3, 0));
}

#[test]
fn the_prior_minor_is_still_readable() {
    // RFC 018 D-7, OQ-2: this build reads 0.2.0 artifacts (the unchanged 0.6.0 layout) as well as 0.3.0.
    let prior = ContractVersion::new(0, 2, 0);
    assert!(READABLE.contains(&prior));
    assert!(prior.accepts());
    assert!(ensure_readable(prior).is_ok());
}

#[test]
fn a_minor_two_behind_is_refused() {
    let too_old = ContractVersion::new(0, CURRENT.minor - 2, 0);
    assert!(!too_old.accepts());
    assert!(ensure_readable(too_old).is_err());
}

#[test]
fn a_non_critical_only_patch_bump_is_still_accepted() {
    // RFC 011 D-3: within the same 0.y, a higher patch (non-critical-only additions) is readable.
    let patched = ContractVersion::new(CURRENT.major, CURRENT.minor, CURRENT.patch + 9);
    assert!(patched.accepts());
    assert!(ensure_readable(patched).is_ok());
}

#[test]
fn an_unlisted_minor_under_major_zero_is_refused() {
    // RFC 011 D-3, widened by RFC 018 D-7: while major 0, a reader accepts only a 0.y in READABLE.
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
