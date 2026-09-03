//! Tests for the IR contract version gate (RFC 003 D-7).

use super::*;

#[test]
fn current_is_readable() {
    assert!(CURRENT.is_readable());
    assert!(ensure_readable(CURRENT).is_ok());
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
