//! Tests for the IR contract version gate (RFC 011 D-3).

use super::*;

#[test]
fn current_is_readable() {
    assert!(CURRENT.accepts());
    assert!(ensure_readable(CURRENT).is_ok());
}

#[test]
fn the_contract_is_0_4_0() {
    // RFC 021 D-2: an entry's content is a blob or a submodule re-cuts the contract to 0.4.0.
    assert_eq!(CURRENT, ContractVersion::new(0, 4, 0));
}

#[test]
fn every_prior_minor_is_still_readable() {
    // RFC 018 D-7 OQ-2 (0.2.0, the unchanged 0.6.0 layout) and RFC 021 D-2 (0.3.0): this build reads
    // both, as well as its own 0.4.0.
    for prior in [ContractVersion::new(0, 2, 0), ContractVersion::new(0, 3, 0)] {
        assert!(READABLE.contains(&prior));
        assert!(prior.accepts());
        assert!(ensure_readable(prior).is_ok());
    }
}

#[test]
fn a_minor_before_the_readable_range_is_refused() {
    let oldest_readable = READABLE.iter().map(|v| v.minor).min().unwrap_or(0);
    let too_old = ContractVersion::new(0, oldest_readable - 1, 0);
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
