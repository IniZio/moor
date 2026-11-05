//! Close wakes an Asleep Box without needing the Proxy tunnel.

use super::boxwork::{fx_woken, put_to_sleep};
use super::{req, row, run, Fx};
use crate::testing::rng::FakeRng;
use moor_core::domain::box_state::BoxLifecycle;

/// Give the Box a Proxy row.
fn with_proxy(f: &Fx) {
    let store = f.cx.store().unwrap();
    store.reserve(f.id, &FakeRng::default()).unwrap();
}

#[test]
fn an_asleep_box_with_a_proxy_is_woken_and_closed() {
    let f = fx_woken("ahead=0 dirty=0");
    put_to_sleep(&f);
    with_proxy(&f);
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    assert_eq!(row(&f).lifecycle, BoxLifecycle::Closed);
    assert!(f.driver.calls().iter().any(|c| c.starts_with("wake")));
}
