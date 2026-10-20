//! Close wakes an Asleep Box without needing the Proxy tunnel.

use std::time::Duration;

use super::boxwork::{fx_woken, put_to_sleep};
use super::{req, row, run, Fx};
use crate::store::{ReadyProc, TunnelAck};
use crate::testing::rng::FakeRng;
use moor_core::domain::box_state::BoxLifecycle;

/// Give the Box a live Proxy: a ready row held by a fresh lease.
fn live_proxy(f: &Fx) {
    let store = f.cx.store().unwrap();
    let lease = Duration::from_secs(30);
    drop(store.reserve(f.id, &FakeRng::default(), lease).unwrap());
    store.claim_lease(f.id, "owner", lease).unwrap();
    let box_port = store.proxy(f.id).unwrap().unwrap().box_port;
    let proc = ReadyProc {
        pid: 1,
        handle: "tunnel-1",
        boot_id: None,
    };
    let _ = store.mark_ready(f.id, "owner", box_port, proc).unwrap();
}

#[test]
fn a_broken_tunnel_does_not_block_closing_an_asleep_box() {
    let f = fx_woken("ahead=0 dirty=0");
    put_to_sleep(&f);
    live_proxy(&f);
    let store = f.cx.store().unwrap();
    let failed = TunnelAck::Failed {
        seq: 1,
        error: "tunnel boom".into(),
    };
    store.ack_tunnel(f.id, &failed).unwrap();
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    assert_eq!(row(&f).lifecycle, BoxLifecycle::Closed);
    assert!(f.driver.calls().iter().any(|c| c.starts_with("wake")));
    assert!(store.tunnel_request(f.id).unwrap().is_none());
}
