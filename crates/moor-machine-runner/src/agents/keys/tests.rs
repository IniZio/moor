#![allow(clippy::unwrap_used, reason = "tests assert on fixed fixtures")]

use serde_json::json;

use super::enrolled_key;

const OWNED: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";
const SHARED: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMKNmEuquBc3TqUA/DQWHHNt+ujMoN27+qMGjIZmpouo";

fn answer() -> serde_json::Value {
    json!({
        "machines": [{"machine_id": 1, "user": 5, "name": "mini", "pubkey": OWNED}],
        "shared": [{"machine_id": 2, "name": "local", "pubkey": SHARED}],
    })
}

#[test]
fn a_users_machine_and_a_shared_machine_each_get_their_own_key() {
    let key = |id| enrolled_key(answer(), id).unwrap().to_openssh().unwrap();
    assert_eq!(key(1), OWNED);
    assert_eq!(key(2), SHARED);
}

#[test]
fn a_machine_in_neither_list_is_not_enrolled() {
    let err = enrolled_key(answer(), 3).unwrap_err();
    assert_eq!(err, "machine 3 is not enrolled");
}

#[test]
fn a_controller_that_lists_no_shared_machines_admits_the_machines_it_lists() {
    let old = json!({"machines": [{"machine_id": 1, "user": 5, "name": "mini", "pubkey": OWNED}]});
    assert!(enrolled_key(old.clone(), 1).is_ok());
    assert_eq!(
        enrolled_key(old, 2).unwrap_err(),
        "machine 2 is not enrolled"
    );
}
