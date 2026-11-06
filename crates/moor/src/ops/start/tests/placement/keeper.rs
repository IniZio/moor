use std::sync::Arc;

use moor_core::domain::box_state::BoxLifecycle;

use super::pressure::{agents_report, tick, wake_up};
use super::*;
use crate::testing::proxy_host::{FakeProxyHost, ProxyHostCall};

/// `w` with a Proxy host that records what it is asked.
fn hosted(mut w: World) -> (World, FakeProxyHost) {
    let host = FakeProxyHost::new();
    w.cx = w.cx.with_proxy_host(Arc::new(host.clone()));
    (w, host)
}

/// The parks and restores `host` was asked for, in order.
fn keeper_calls(host: &FakeProxyHost) -> Vec<ProxyHostCall> {
    host.calls()
        .into_iter()
        .filter(|c| matches!(c, ProxyHostCall::Park(_) | ProxyHostCall::Restore(_)))
        .collect()
}

#[test]
fn a_pressure_suspend_of_an_idle_box_parks_its_keeper_and_a_wake_restores_it() {
    let (w, hv) = live_world(32_768);
    let (a, b) = (started(&w, "a"), started(&w, "b"));
    idle(&w, a);
    idle(&w, b);
    let (w, _) = agents_report(w, &[a, b], &[]);
    let (w, host) = hosted(w);
    for _ in 0..3 {
        tick(&w, hv, 32_768, 1000, 0);
    }
    assert_eq!(lifecycle(&w, a), BoxLifecycle::Asleep);
    assert_eq!(keeper_calls(&host), [ProxyHostCall::Park(a)]);
    tick(&w, hv, 32_768, 0, 0);
    wake_up(&w, a).unwrap();
    assert_eq!(
        keeper_calls(&host),
        [ProxyHostCall::Park(a), ProxyHostCall::Restore(a)]
    );
}

#[test]
fn a_last_resort_suspend_parks_the_keeper_and_its_resume_restores_it() {
    let (w, hv) = live_world(32_768);
    let (b, c) = (started(&w, "b"), started(&w, "c"));
    let (w, _) = agents_report(w, &[], &[b, c]);
    let (w, host) = hosted(w);
    tick(&w, hv, 512, 0, 0);
    assert_eq!(lifecycle(&w, c), BoxLifecycle::Pending);
    assert_eq!(keeper_calls(&host), [ProxyHostCall::Park(c)]);
    tick(&w, hv, 32_768, 0, 0);
    assert_eq!(lifecycle(&w, c), BoxLifecycle::Ready);
    assert_eq!(
        keeper_calls(&host),
        [ProxyHostCall::Park(c), ProxyHostCall::Restore(c)]
    );
}
