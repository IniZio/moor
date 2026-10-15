//! Waking a Box brings the Task's Proxy tunnel back before the op proceeds.

use std::time::Duration;

use moor_core::domain::boxes::BoxState;

use super::{fx, row, sleep, wake_if_asleep, Fx};
use crate::store::{ProxyPorts, ReadyProc, TunnelAck, TunnelWant};
use crate::testing::rng::FakeRng;

/// Give the Task a live Proxy: a ready row held by a fresh lease.
fn live_proxy(f: &Fx) {
    let store = f.cx.store().unwrap();
    let lease = Duration::from_secs(30);
    let nonce = moor_core::secret::Secret::new(b"nonce".to_vec());
    drop(
        store
            .reserve(f.id, &nonce, &FakeRng::default(), lease)
            .unwrap(),
    );
    store.claim_lease(f.id, "owner", lease).unwrap();
    let box_port = store.proxy(f.id).unwrap().unwrap().box_port;
    let ports = ProxyPorts {
        box_port,
        host_port: 41_000,
    };
    let proc = ReadyProc {
        pid: 1,
        handle: "tunnel-1",
        boot_id: None,
    };
    let _ = store.mark_ready(f.id, "owner", ports, proc).unwrap();
}

/// The woken Box is recorded awake.
fn box_state(f: &Fx) -> BoxState {
    row(f).boxed.unwrap().state
}

#[test]
fn wake_asks_the_supervisor_to_bring_the_tunnel_up() {
    let f = fx();
    sleep(&f);
    live_proxy(&f);
    let store = f.cx.store().unwrap();
    store.ack_tunnel(f.id, &TunnelAck::Up { seq: 1 }).unwrap();
    assert!(wake_if_asleep(&f.cx, store, &row(&f)).unwrap());
    let req = store.tunnel_request(f.id).unwrap().unwrap();
    assert_eq!((req.want, req.seq), (TunnelWant::Up, 1));
    assert_eq!(box_state(&f), BoxState::Ready);
}

#[test]
fn tunnel_failure_fails_the_wake_and_leaves_the_box_awake() {
    let f = fx();
    sleep(&f);
    live_proxy(&f);
    let store = f.cx.store().unwrap();
    let failed = TunnelAck::Failed {
        seq: 1,
        error: "tunnel boom".into(),
    };
    store.ack_tunnel(f.id, &failed).unwrap();
    let err = wake_if_asleep(&f.cx, store, &row(&f)).unwrap_err();
    assert!(err.error.to_string().contains("tunnel boom"), "{err:?}");
    assert_eq!(box_state(&f), BoxState::Ready);
}

#[test]
fn wake_without_a_proxy_makes_no_tunnel_request() {
    let f = fx();
    sleep(&f);
    let store = f.cx.store().unwrap();
    assert!(wake_if_asleep(&f.cx, store, &row(&f)).unwrap());
    assert!(store.tunnel_request(f.id).unwrap().is_none());
}
