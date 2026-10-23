//! Tests for leases of a front's Proxy.

use std::time::Duration;

use moor_core::seam::clock::Clock as _;
use moor_core::user::Identity;

use super::hash;
use crate::store::tests::open;

#[test]
fn a_lease_serves_only_its_front_until_it_expires() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    let (user, _) = store
        .enroll(&Identity::parse("slack:T1:U1").unwrap())
        .unwrap();
    let expires = clock.now().saturating_add(Duration::from_secs(60));
    store
        .add_front_lease(b"h1", "slack", user, expires)
        .unwrap();
    assert_eq!(store.front_lease_user(b"h1", "slack").unwrap(), Some(user));
    assert_eq!(
        store.front_lease_user(b"h1", "web").unwrap(),
        None,
        "another front's"
    );
    assert_eq!(
        store.front_lease_user(b"h2", "slack").unwrap(),
        None,
        "unknown token"
    );
    clock.sleep(Duration::from_secs(60));
    assert_eq!(
        store.front_lease_user(b"h1", "slack").unwrap(),
        None,
        "expired"
    );
}

#[test]
fn release_and_purge_end_leases() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    let (user, _) = store
        .enroll(&Identity::parse("slack:T1:U1").unwrap())
        .unwrap();
    let (other, _) = store
        .enroll(&Identity::parse("slack:T1:U2").unwrap())
        .unwrap();
    let expires = clock.now().saturating_add(Duration::from_secs(60));
    store
        .add_front_lease(b"h1", "slack", user, expires)
        .unwrap();
    store
        .add_front_lease(b"h2", "slack", user, expires)
        .unwrap();
    assert!(
        !store.delete_front_lease(b"h1", "slack", other).unwrap(),
        "not the owner"
    );
    assert!(store.delete_front_lease(b"h1", "slack", user).unwrap());
    assert_eq!(store.front_lease_user(b"h1", "slack").unwrap(), None);
    store.purge_front_leases().unwrap();
    assert_eq!(store.front_lease_user(b"h2", "slack").unwrap(), None);
}

#[test]
fn the_hash_is_hex_sha256() {
    assert_eq!(
        hash(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
