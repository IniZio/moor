//! Closing a task asks the proxy host to stop its proxy; a failure only warns.

use std::sync::Arc;

use moor_core::seam::proxy_host::ProxyHostError;

use super::{fx, req, run, Fx};
use crate::testing::proxy_host::{FakeProxyHost, ProxyHostCall};

/// A fixture whose context stops proxies through `fake`.
fn with_host(fake: &FakeProxyHost) -> Fx {
    let mut f = fx();
    f.cx = f.cx.with_proxy_host(Arc::new(fake.clone()));
    f
}

#[test]
fn close_stops_the_task_proxy_once_without_a_warning() {
    let fake = FakeProxyHost::new();
    let f = with_host(&fake);

    let v = run(&f.cx, &req(false)).unwrap();

    assert_eq!(v.task.state.as_str(), "closed");
    assert_eq!(fake.calls(), [ProxyHostCall::Stop(f.id)]);
    assert!(f.cx.take_warnings().is_empty());
}

#[test]
fn an_incomplete_stop_only_warns() {
    let reason = "the proxy supervisor did not exit within 3s";
    let fake = FakeProxyHost::new().with_stop_error(ProxyHostError::Incomplete(reason.into()));
    let f = with_host(&fake);

    let v = run(&f.cx, &req(false)).unwrap();

    assert_eq!(v.task.state.as_str(), "closed");
    assert_eq!(fake.calls(), [ProxyHostCall::Stop(f.id)]);
    let warnings = f.cx.take_warnings();
    let stops: Vec<_> = warnings
        .iter()
        .filter(|w| w.code == "proxy_stop_failed")
        .collect();
    assert_eq!(stops.len(), 1);
    assert_eq!(
        stops.first().unwrap().message,
        "the proxy supervisor did not exit within 3s; the task is closed anyway"
    );
}
