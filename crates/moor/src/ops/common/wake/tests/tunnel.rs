//! Waking a Box brings its tunnel back before the op proceeds.

use std::sync::Arc;

use moor_core::domain::boxes::BoxState;
use moor_core::seam::proxy_host::ProxyHostError;

use super::{fx, row, sleep, wake_and_revive, wake_box_if_asleep, wake_if_asleep, with_agent, Fx};
use crate::testing::proxy_host::{FakeProxyHost, ProxyHostCall};

/// The woken Box is recorded awake.
fn box_state(f: &Fx) -> BoxState {
    row(f).boxed.unwrap().state
}

/// `f` with `host` as its Proxy host.
fn hosted(mut f: Fx, host: &FakeProxyHost) -> Fx {
    f.cx = f.cx.with_proxy_host(Arc::new(host.clone()));
    f
}

/// A host whose restore fails with `tunnel boom`.
fn failing() -> FakeProxyHost {
    FakeProxyHost::new().with_ensure_error(ProxyHostError::Failed("tunnel boom".into()))
}

#[test]
fn wake_restores_the_tunnel() {
    let host = FakeProxyHost::new();
    let f = hosted(fx(), &host);
    sleep(&f);
    let store = f.cx.store().unwrap();
    assert!(wake_if_asleep(&f.cx, store, &row(&f)).unwrap());
    assert!(host.calls().contains(&ProxyHostCall::Restore(f.id)));
    assert_eq!(box_state(&f), BoxState::Ready);
}

#[test]
fn tunnel_failure_fails_the_wake_and_leaves_the_box_awake() {
    let f = hosted(fx(), &failing());
    sleep(&f);
    let store = f.cx.store().unwrap();
    let err = wake_if_asleep(&f.cx, store, &row(&f)).unwrap_err();
    assert!(err.error.to_string().contains("tunnel boom"), "{err:?}");
    assert_eq!(box_state(&f), BoxState::Ready);
}

#[test]
fn the_tunnel_is_up_before_the_agent_relaunches() {
    let host = FakeProxyHost::new();
    let f = hosted(fx(), &host);
    with_agent(&f);
    sleep(&f);
    f.engine
        .set_probes("agent-1", crate::testing::agent::gone());
    let store = f.cx.store().unwrap();
    assert!(wake_and_revive(&f.cx, store, &row(&f)).unwrap());
    assert!(host.calls().contains(&ProxyHostCall::Restore(f.id)));
    assert_eq!(f.engine.launches().len(), 1);
}

#[test]
fn tunnel_failure_attempts_no_relaunch() {
    let f = hosted(fx(), &failing());
    with_agent(&f);
    sleep(&f);
    f.engine
        .set_probes("agent-1", crate::testing::agent::gone());
    let store = f.cx.store().unwrap();
    let err = wake_and_revive(&f.cx, store, &row(&f)).unwrap_err();
    assert!(err.error.to_string().contains("tunnel boom"), "{err:?}");
    assert_eq!(box_state(&f), BoxState::Ready);
    assert!(f.engine.launches().is_empty());
}

#[test]
fn waking_only_the_box_leaves_the_tunnel_and_the_agent_alone() {
    let host = FakeProxyHost::new();
    let f = hosted(fx(), &host);
    with_agent(&f);
    sleep(&f);
    f.engine
        .set_probes("agent-1", crate::testing::agent::gone());
    let store = f.cx.store().unwrap();
    assert!(wake_box_if_asleep(&f.cx, store, &row(&f)).unwrap());
    assert!(!host.calls().contains(&ProxyHostCall::Restore(f.id)));
    assert!(f.engine.launches().is_empty());
    assert_eq!(box_state(&f), BoxState::Ready);
}
