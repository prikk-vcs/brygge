//! Tests for the IR contract version gate (RFC 003 D-7).

use super::*;

#[test]
fn current_is_readable() {
    assert!(CURRENT.is_readable());
    assert!(ensure_readable(CURRENT).is_ok());
}

#[test]
fn the_contract_is_frozen_at_1_0() {
    // RFC 003 D-7: the freeze declared the contract 1.0. A regression to 0.x would silently reopen it.
    assert_eq!(CURRENT, ContractVersion::new(1, 0, 0));
}

#[test]
fn a_pre_freeze_major_zero_artifact_stays_readable() {
    // Backward compatibility across the freeze: a 0.x artifact is still readable under frozen major 1.
    assert!(ContractVersion::new(0, 1, 0).is_readable());
    assert!(ensure_readable(ContractVersion::new(0, 1, 0)).is_ok());
}

#[test]
fn an_older_major_is_readable_a_newer_major_is_not() {
    let older = ContractVersion::new(CURRENT.major, CURRENT.minor + 9, CURRENT.patch);
    assert!(older.is_readable()); // same major, newer minor — additive forward-compat
    let newer_major = ContractVersion::new(CURRENT.major + 1, 0, 0);
    assert!(!newer_major.is_readable());
    match ensure_readable(newer_major) {
        Err(crate::Error::UnsupportedContractMajor { found, supported }) => {
            assert_eq!(found, CURRENT.major + 1);
            assert_eq!(supported, CURRENT.major);
        }
        other => panic!("expected UnsupportedContractMajor, got {other:?}"),
    }
}
