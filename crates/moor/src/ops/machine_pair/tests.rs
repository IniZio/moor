//! Tests for `machine-pair`.

use moor_core::user::Caller;

use super::permit;

#[test]
fn only_a_front_may_pair_machines() {
    assert!(permit(&Ok(Caller::Front("gate".to_owned()))).is_ok());
    assert!(permit(&Ok(Caller::Runner)).is_err());
    assert!(permit(&Err("unknown".to_owned())).is_err());
}
