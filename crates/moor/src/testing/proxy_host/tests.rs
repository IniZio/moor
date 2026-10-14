//! Tests for the proxy host fake.

use std::path::PathBuf;

use moor_core::seam::proxy_host::{ProxyHost, ProxyHostError, ProxySpec, ProxyStatus};
use moor_core::TaskId;

use super::{FakeProxyHost, ProxyHostCall, FAKE_BOX_PORT, FAKE_NONCE};

fn spec(id: i64) -> ProxySpec {
    ProxySpec {
        task: TaskId::new(id),
        box_handle: None,
        host_cwd: PathBuf::from("/work"),
        task_repo: None,
    }
}

#[test]
fn records_calls_in_order_across_clones() {
    let fake = FakeProxyHost::new();
    let host: &dyn ProxyHost = &fake.clone();
    host.ensure(&spec(1)).expect("ensure");
    assert_eq!(host.status(TaskId::new(1)), Ok(ProxyStatus::Absent));
    host.stop(TaskId::new(1)).expect("stop");
    assert_eq!(
        fake.calls(),
        vec![
            ProxyHostCall::Ensure(spec(1)),
            ProxyHostCall::Status(TaskId::new(1)),
            ProxyHostCall::Stop(TaskId::new(1)),
        ]
    );
}

#[test]
fn default_endpoint_is_fresh_each_ensure() {
    let fake = FakeProxyHost::new();
    let a = fake.ensure(&spec(1)).expect("ensure");
    let b = fake.ensure(&spec(1)).expect("ensure");
    assert_eq!(a.box_port, FAKE_BOX_PORT);
    assert_eq!(a.nonce.expose(), FAKE_NONCE);
    assert_eq!(b.nonce.expose(), FAKE_NONCE);
}

#[test]
fn scripted_endpoint_and_status() {
    let fake = FakeProxyHost::new()
        .with_endpoint(7, b"abc")
        .with_status(ProxyStatus::Ready { box_port: 7 });
    let ep = fake.ensure(&spec(2)).expect("ensure");
    assert_eq!((ep.box_port, ep.nonce.expose()), (7, &b"abc"[..]));
    assert_eq!(
        fake.status(TaskId::new(2)),
        Ok(ProxyStatus::Ready { box_port: 7 })
    );
}

#[test]
fn scripted_errors_are_returned_and_recorded() {
    let fake = FakeProxyHost::new()
        .with_ensure_error(ProxyHostError::Failed("boom".into()))
        .with_stop_error(ProxyHostError::Incomplete("slow".into()));
    assert_eq!(
        fake.ensure(&spec(3)).map(|_| ()),
        Err(ProxyHostError::Failed("boom".into()))
    );
    assert_eq!(
        fake.stop(TaskId::new(3)),
        Err(ProxyHostError::Incomplete("slow".into()))
    );
    assert_eq!(fake.calls().len(), 2);
}

#[test]
fn debug_hides_nonce() {
    let text = format!("{:?}", FakeProxyHost::new());
    assert!(!text.contains("fake-proxy-nonce"));
}
