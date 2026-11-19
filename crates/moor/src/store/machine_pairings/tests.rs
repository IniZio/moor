//! Tests for the codes machines pair by.

use std::time::Duration;

use moor_core::seam::clock::Clock as _;

use super::{Pairing, Polled};
use crate::store::tests::open;

const TTL: Duration = Duration::from_mins(10);

fn pairing(code: &'static str, device_hash: &'static str) -> Pairing<'static> {
    Pairing {
        code,
        device_hash,
        ask: r#"{"name":"hv"}"#,
        ttl: TTL,
    }
}

#[test]
fn a_waiting_code_is_claimed_by_one_confirm() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    store
        .start_pairing(&pairing("ABCD-FGHJ", "d"), 256)
        .unwrap();
    assert_eq!(
        store.waiting_pairing("ABCD-FGHJ").unwrap().as_deref(),
        Some(r#"{"name":"hv"}"#)
    );
    assert!(store.claim_pairing("ABCD-FGHJ").unwrap().is_some());
    assert!(
        store.claim_pairing("ABCD-FGHJ").unwrap().is_none(),
        "a second confirm gets nothing"
    );
    assert!(store.waiting_pairing("ABCD-FGHJ").unwrap().is_none());
}

#[test]
fn a_confirmed_code_hands_its_enrollment_to_one_poll() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    assert_eq!(
        store
            .start_pairing(&pairing("ABCD-FGHJ", "d"), 256)
            .unwrap(),
        Some(true)
    );
    assert_eq!(store.poll_pairing("d").unwrap(), Polled::Pending);
    store.claim_pairing("ABCD-FGHJ").unwrap().unwrap();
    assert_eq!(store.poll_pairing("d").unwrap(), Polled::Pending);
    assert!(store.settle_pairing("ABCD-FGHJ", Some("{}")).unwrap());
    assert_eq!(
        store.poll_pairing("d").unwrap(),
        Polled::Enrolled("{}".to_owned())
    );
    assert_eq!(store.poll_pairing("d").unwrap(), Polled::Expired);
}

#[test]
fn a_code_past_its_ten_minutes_can_be_neither_confirmed_nor_collected() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    store
        .start_pairing(&pairing("ABCD-FGHJ", "d"), 256)
        .unwrap();
    clock.sleep(TTL);
    assert!(store.waiting_pairing("ABCD-FGHJ").unwrap().is_none());
    assert!(store.claim_pairing("ABCD-FGHJ").unwrap().is_none());
    assert_eq!(store.poll_pairing("d").unwrap(), Polled::Expired);
}

#[test]
fn an_answer_confirmed_in_time_but_collected_late_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    store
        .start_pairing(&pairing("ABCD-FGHJ", "d"), 256)
        .unwrap();
    store.claim_pairing("ABCD-FGHJ").unwrap().unwrap();
    assert!(store.settle_pairing("ABCD-FGHJ", Some("{}")).unwrap());
    clock.sleep(TTL);
    assert_eq!(store.poll_pairing("d").unwrap(), Polled::Expired);
}

#[test]
fn a_failed_enroll_leaves_the_code_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    store
        .start_pairing(&pairing("ABCD-FGHJ", "d"), 256)
        .unwrap();
    store.claim_pairing("ABCD-FGHJ").unwrap().unwrap();
    assert!(store.settle_pairing("ABCD-FGHJ", None).unwrap());
    assert!(store.waiting_pairing("ABCD-FGHJ").unwrap().is_some());
    assert_eq!(store.poll_pairing("d").unwrap(), Polled::Pending);
}

#[test]
fn a_code_or_device_hash_in_use_is_not_kept_twice() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    store
        .start_pairing(&pairing("ABCD-FGHJ", "d"), 256)
        .unwrap();
    assert_eq!(
        store
            .start_pairing(&pairing("ABCD-FGHJ", "e"), 256)
            .unwrap(),
        Some(false)
    );
    assert_eq!(
        store
            .start_pairing(&pairing("BCDF-GHJK", "d"), 256)
            .unwrap(),
        Some(false)
    );
}

#[test]
fn expired_codes_make_room_for_new_ones() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    store.start_pairing(&pairing("ABCD-FGHJ", "d"), 2).unwrap();
    store.start_pairing(&pairing("BCDF-GHJK", "e"), 2).unwrap();
    assert_eq!(
        store.start_pairing(&pairing("CDFG-HJKL", "f"), 2).unwrap(),
        None
    );
    clock.sleep(TTL);
    assert_eq!(
        store.start_pairing(&pairing("CDFG-HJKL", "f"), 2).unwrap(),
        Some(true)
    );
}

const ALICE: (&str, &str) = ("authgear:alice", r#"{"who":"alice"}"#);

#[test]
fn a_grant_is_redeemed_once() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    assert!(store.grant_pairing(ALICE, "t1", TTL, 256).unwrap());
    assert_eq!(
        store.redeem_pairing("t1").unwrap().as_deref(),
        Some(ALICE.1)
    );
    assert_eq!(store.redeem_pairing("t1").unwrap(), None);
}

#[test]
fn a_grant_is_never_shown_claimed_settled_or_polled_as_a_code() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    store.grant_pairing(ALICE, "t1", TTL, 256).unwrap();
    let code = "grant:authgear:alice";
    assert!(store.waiting_pairing(code).unwrap().is_none());
    assert!(store.claim_pairing(code).unwrap().is_none());
    assert!(!store.settle_pairing(code, Some("{}")).unwrap());
    assert!(!store.settle_pairing(code, None).unwrap());
    assert_eq!(store.poll_pairing("t1").unwrap(), Polled::Expired);
    assert!(store.redeem_pairing("t1").unwrap().is_some());
}

#[test]
fn a_code_is_not_redeemable_as_a_grant() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    store
        .start_pairing(&pairing("ABCD-FGHJ", "d"), 256)
        .unwrap();
    assert_eq!(store.redeem_pairing("d").unwrap(), None);
    assert_eq!(store.poll_pairing("d").unwrap(), Polled::Pending);
}

#[test]
fn a_new_grant_replaces_the_owners_earlier_one() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    store.grant_pairing(ALICE, "t1", TTL, 256).unwrap();
    store.grant_pairing(ALICE, "t2", TTL, 256).unwrap();
    assert_eq!(store.redeem_pairing("t1").unwrap(), None);
    assert!(store.redeem_pairing("t2").unwrap().is_some());
}

#[test]
fn a_grant_past_its_ttl_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    store.grant_pairing(ALICE, "t1", TTL, 256).unwrap();
    clock.sleep(TTL);
    assert_eq!(store.redeem_pairing("t1").unwrap(), None);
}

#[test]
fn a_full_store_refuses_a_new_owners_grant_but_not_a_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    store.grant_pairing(ALICE, "t1", TTL, 2).unwrap();
    store.start_pairing(&pairing("ABCD-FGHJ", "d"), 2).unwrap();
    assert!(!store
        .grant_pairing(("authgear:bob", "{}"), "t2", TTL, 2)
        .unwrap());
    assert!(store.grant_pairing(ALICE, "t3", TTL, 2).unwrap());
}
