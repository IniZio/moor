//! Tests for parking and restoring the Box's Proxy tunnel.

use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::decision::{Decision, NewBox};
use moor_core::seam::clock::{Clock, Ts};
use moor_core::{BoxId, BoxName};

use super::{park_tunnel, restore_tunnel, LEASE};
use crate::cx::Cx;
use crate::store::{ReadyProc, TunnelAck, TunnelWant};
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::rng::FakeRng;
use crate::testing::surface::FakeSurface;

/// A context over a fresh store, the clock it runs on and one Box without a proxy row.
struct Fx {
    _dir: tempfile::TempDir,
    cx: Cx,
    clock: Arc<FakeClock>,
    id: BoxId,
}

/// A fixture whose Box has no proxy row.
fn fx() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let cx = Cx::fake_with_clock(
        dir.path(),
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
        moor_core::config::UserConfig::default(),
        clock.clone(),
    );
    let id = cx
        .store()
        .unwrap()
        .apply(Decision::insert(NewBox {
            name: BoxName::parse_allow_bare("t").unwrap(),
            repo: None,
        }))
        .unwrap()
        .box_id;
    Fx {
        _dir: dir,
        cx,
        clock,
        id,
    }
}

/// A fixture whose proxy row is ready under a supervisor that just heartbeat.
fn live() -> Fx {
    let f = fx();
    let store = f.cx.store().unwrap();
    drop(store.reserve(f.id, &FakeRng::default(), LEASE).unwrap());
    drop(store.claim_lease(f.id, "sup", LEASE).unwrap());
    let box_port = store.proxy(f.id).unwrap().unwrap().box_port;
    let proc = ReadyProc {
        pid: 1,
        handle: "tunnel-1",
        boot_id: None,
    };
    let _ = store.mark_ready(f.id, "sup", box_port, proc).unwrap();
    f
}

#[test]
fn no_proxy_row_needs_no_tunnel_work() {
    let f = fx();
    let store = f.cx.store().unwrap();
    park_tunnel(&f.cx, store, f.id).unwrap();
    restore_tunnel(&f.cx, store, f.id, false).unwrap();
    assert_eq!(store.tunnel_request(f.id).unwrap(), None);
}

#[test]
fn stale_lease_is_not_asked() {
    let f = live();
    f.clock.sleep(LEASE + Duration::from_secs(1));
    let store = f.cx.store().unwrap();
    park_tunnel(&f.cx, store, f.id).unwrap();
    assert_eq!(store.tunnel_request(f.id).unwrap(), None);
}

#[test]
fn unready_proxy_is_not_asked() {
    let f = fx();
    let store = f.cx.store().unwrap();
    drop(store.reserve(f.id, &FakeRng::default(), LEASE).unwrap());
    drop(store.claim_lease(f.id, "sup", LEASE).unwrap());
    park_tunnel(&f.cx, store, f.id).unwrap();
    assert_eq!(store.tunnel_request(f.id).unwrap(), None);
}

#[test]
fn park_writes_the_request_and_accepts_the_parked_ack() {
    let f = live();
    let store = f.cx.store().unwrap();
    store
        .ack_tunnel(f.id, &TunnelAck::Parked { seq: 1 })
        .unwrap();
    park_tunnel(&f.cx, store, f.id).unwrap();
    let req = store.tunnel_request(f.id).unwrap().unwrap();
    assert_eq!((req.want, req.seq), (TunnelWant::Parked, 1));
}

#[test]
fn restore_writes_the_request_and_accepts_the_up_ack() {
    let f = live();
    let store = f.cx.store().unwrap();
    store.ack_tunnel(f.id, &TunnelAck::Up { seq: 1 }).unwrap();
    restore_tunnel(&f.cx, store, f.id, false).unwrap();
    let req = store.tunnel_request(f.id).unwrap().unwrap();
    assert_eq!((req.want, req.seq), (TunnelWant::Up, 1));
}

#[test]
fn restore_failure_carries_the_tunnel_error() {
    let f = live();
    let store = f.cx.store().unwrap();
    let failed = TunnelAck::Failed {
        seq: 1,
        error: "reverse tunnel refused".to_owned(),
    };
    store.ack_tunnel(f.id, &failed).unwrap();
    let err = restore_tunnel(&f.cx, store, f.id, false).unwrap_err();
    assert!(err.error.to_string().contains("reverse tunnel refused"));
}

#[test]
fn park_fails_on_a_failed_or_wrong_ack() {
    let f = live();
    let store = f.cx.store().unwrap();
    store.ack_tunnel(f.id, &TunnelAck::Up { seq: 1 }).unwrap();
    assert!(park_tunnel(&f.cx, store, f.id).is_err());
}

#[test]
fn park_times_out_without_an_ack() {
    let f = live();
    let store = f.cx.store().unwrap();
    // An answer to an older request does not count.
    store
        .ack_tunnel(f.id, &TunnelAck::Parked { seq: 0 })
        .unwrap();
    let before = f.clock.instant();
    let err = park_tunnel(&f.cx, store, f.id).unwrap_err();
    assert!(err.error.to_string().contains("did not park the tunnel"));
    assert!(f.clock.instant() - before >= Duration::from_secs(10));
}

#[test]
fn restore_times_out_without_an_ack() {
    let f = live();
    let store = f.cx.store().unwrap();
    let err = restore_tunnel(&f.cx, store, f.id, false).unwrap_err();
    assert!(err.error.to_string().contains("did not bring the tunnel"));
}
