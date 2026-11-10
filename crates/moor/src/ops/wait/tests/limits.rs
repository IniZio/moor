//! Tests for `wait` on a Box with an open limit.

use moor_core::domain::event::Event;
use moor_core::{BoxId, ErrorKind, WaitFor};

use super::{add, add_agentless, cx, req};
use crate::cx::Cx;
use crate::ops::wait::run;

/// Record an `agent_rate_limited` event for the `claude` rule on `Box`.
fn rate_limit(cx: &Cx, box_id: BoxId) {
    cx.store()
        .unwrap()
        .record_proxy_event(
            box_id,
            &Event::AgentRateLimited {
                rule: "claude".into(),
                status: 429,
                resets_at: None,
            },
        )
        .unwrap();
}

#[test]
fn an_open_rate_limit_fails_the_wait_with_attention() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _) = cx(dir.path());
    let id = add(&cx);
    rate_limit(&cx, id);
    let e = run(&cx, &req(None, None)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert_eq!(e.error.to_string(), "Claude account rate limited (429)");
    let shown = e.value.unwrap();
    assert_eq!(
        shown.pointer("/attention/0/kind").and_then(|v| v.as_str()),
        Some("agent_rate_limited")
    );
    assert_eq!(
        shown
            .pointer("/attention/0/status")
            .and_then(serde_json::Value::as_u64),
        Some(429)
    );
}

#[test]
fn a_recovered_rate_limit_no_longer_fails_the_wait() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _) = cx(dir.path());
    let id = add_agentless(&cx);
    rate_limit(&cx, id);
    cx.store()
        .unwrap()
        .record_proxy_event(
            id,
            &Event::ProxyCredentialRecovered {
                rule: "claude".into(),
            },
        )
        .unwrap();
    let v = run(&cx, &req(None, None)).unwrap();
    assert!(!v.timed_out);
    assert!(v.boxed.attention.is_empty());
}

#[test]
fn waiting_for_the_run_ignores_open_limits() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _) = cx(dir.path());
    let id = add(&cx);
    rate_limit(&cx, id);
    let v = run(&cx, &req(Some(WaitFor::Run), None)).unwrap();
    assert_eq!(v.wait_for, WaitFor::Run);
}
