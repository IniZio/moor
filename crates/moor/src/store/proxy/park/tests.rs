//! Tests for tunnel park requests and acks.

use std::path::Path;
use std::sync::Arc;

use moor_core::domain::decision::{Decision, NewBox};
use moor_core::seam::clock::Ts;
use moor_core::{BoxId, BoxName};

use super::super::Reserve;
use super::{TunnelAck, TunnelRequest, TunnelWant};
use crate::store::{Store, StoreError};
use crate::testing::clock::FakeClock;
use crate::testing::rng::FakeRng;

/// Open a store in a temp dir.
fn open(dir: &Path) -> Store {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    Store::open(&dir.join("moor.db"), clock).unwrap()
}

/// Insert a box by name.
fn add(store: &Store, name: &str) -> BoxId {
    let nt = NewBox {
        name: BoxName::parse_allow_bare(name).unwrap(),
        repo: "r".into(),
    };
    store.apply(Decision::insert(nt)).unwrap().box_id
}

#[test]
fn unset_reads_none() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    assert_eq!(store.tunnel_request(t).unwrap(), None);
    assert_eq!(store.tunnel_ack(t).unwrap(), None);
}

#[test]
fn seq_starts_at_one_and_increments_across_wants() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    assert_eq!(store.request_tunnel(t, TunnelWant::Parked).unwrap(), 1);
    assert_eq!(
        store.tunnel_request(t).unwrap(),
        Some(TunnelRequest {
            want: TunnelWant::Parked,
            seq: 1
        })
    );
    assert_eq!(store.request_tunnel(t, TunnelWant::Up).unwrap(), 2);
    assert_eq!(store.request_tunnel(t, TunnelWant::Up).unwrap(), 3);
    assert_eq!(
        store.tunnel_request(t).unwrap(),
        Some(TunnelRequest {
            want: TunnelWant::Up,
            seq: 3
        })
    );
}

#[test]
fn acks_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    for ack in [TunnelAck::Parked { seq: 4 }, TunnelAck::Up { seq: 5 }] {
        store.ack_tunnel(t, &ack).unwrap();
        assert_eq!(store.tunnel_ack(t).unwrap(), Some(ack.clone()));
        assert_eq!(ack.seq(), store.tunnel_ack(t).unwrap().unwrap().seq());
    }
}

#[test]
fn failed_error_round_trips_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    for error in [
        "",
        "ssh exited 255",
        "line one\nline two  \n",
        " lead and  double  spaces ",
    ] {
        let ack = TunnelAck::Failed {
            seq: 9,
            error: error.into(),
        };
        store.ack_tunnel(t, &ack).unwrap();
        assert_eq!(store.tunnel_ack(t).unwrap(), Some(ack));
    }
}

#[test]
fn remove_proxy_removes_both() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    let rng = FakeRng::default();
    let outcome = store
        .reserve(t, &rng, std::time::Duration::from_secs(30))
        .unwrap();
    assert!(matches!(outcome, Reserve::Reserved(_)), "{outcome:?}");
    store.request_tunnel(t, TunnelWant::Parked).unwrap();
    store.ack_tunnel(t, &TunnelAck::Parked { seq: 1 }).unwrap();
    assert!(store.remove_proxy(t).unwrap());
    assert_eq!(store.tunnel_request(t).unwrap(), None);
    assert_eq!(store.tunnel_ack(t).unwrap(), None);
    assert_eq!(store.request_tunnel(t, TunnelWant::Up).unwrap(), 1);
}

#[test]
fn boxes_are_isolated() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let a = add(&store, "a");
    let b = add(&store, "b");
    store.request_tunnel(a, TunnelWant::Parked).unwrap();
    store.request_tunnel(a, TunnelWant::Up).unwrap();
    store.ack_tunnel(a, &TunnelAck::Up { seq: 2 }).unwrap();
    assert_eq!(store.tunnel_request(b).unwrap(), None);
    assert_eq!(store.tunnel_ack(b).unwrap(), None);
    assert_eq!(store.request_tunnel(b, TunnelWant::Parked).unwrap(), 1);
    assert_eq!(store.tunnel_ack(a).unwrap(), Some(TunnelAck::Up { seq: 2 }));
}

#[test]
fn unparsable_values_are_corrupt() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    let conn = rusqlite::Connection::open(dir.path().join("moor.db")).unwrap();
    for (k, v) in [
        (format!("tunnel_want:{}", t.get()), "sideways 1"),
        (format!("tunnel_ack:{}", t.get()), "up x"),
    ] {
        conn.execute("INSERT INTO meta (k, v) VALUES (?, ?)", [k, v.to_owned()])
            .unwrap();
    }
    drop(conn);
    assert!(matches!(
        store.tunnel_request(t),
        Err(StoreError::Corrupt(_))
    ));
    assert!(matches!(store.tunnel_ack(t), Err(StoreError::Corrupt(_))));
    assert!(matches!(
        store.request_tunnel(t, TunnelWant::Up),
        Err(StoreError::Corrupt(_))
    ));
}
