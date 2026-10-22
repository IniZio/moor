//! Tests for follow claims.

use super::{held, held_between, FollowClaim};

#[test]
fn a_claim_locked_after_its_file_was_replaced_is_not_a_second_holder() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runs/7.follow");
    let mut first = Some(FollowClaim::take(path.clone()).unwrap());
    // The second taker opens the file, then the first holder removes it and lets go.
    let second = FollowClaim::take_between(path.clone(), &mut || drop(first.take())).unwrap();

    assert!(held(&path).unwrap(), "the second taker's claim is held");
    assert!(
        FollowClaim::take(path.clone()).is_err(),
        "a third taker is refused while the second holds the claim"
    );
    drop(second);
    assert!(!held(&path).unwrap());
    assert!(!path.exists(), "the file goes with the last claim");
}

#[test]
fn a_probe_of_a_replaced_file_judges_the_file_now_at_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runs/7.follow");
    let mut first = Some(FollowClaim::take(path.clone()).unwrap());
    let mut second = None;
    // The probe opens the file, then the holder lets go and a new taker claims a new file.
    let seen = held_between(&path, &mut || {
        if let Some(claim) = first.take() {
            drop(claim);
            second = Some(FollowClaim::take(path.clone()).unwrap());
        }
    })
    .unwrap();

    assert!(seen, "the new taker holds the claim");
    assert!(second.is_some());
}
