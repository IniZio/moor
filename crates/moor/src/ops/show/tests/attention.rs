//! Tests for the attention section of `show`.

use std::sync::Arc;

use moor_core::domain::event::Event;
use moor_core::view::BoxView;

use super::super::run;
use super::{add, cx, req};
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;

/// Record an event for Box `a`'s id.
fn record(cx: &Cx, id: moor_core::BoxId, event: &Event) {
    cx.store().unwrap().record_proxy_event(id, event).unwrap();
}

/// The rate-limit event for the `claude` rule.
fn limited() -> Event {
    Event::AgentRateLimited {
        rule: "claude".into(),
        status: 429,
        resets_at: None,
    }
}

/// The attention kind and message of `view`.
fn first_attention(view: &BoxView) -> (String, String) {
    let a = view.attention.first().expect("attention");
    (a.kind.clone(), a.message.clone())
}

#[test]
fn rate_limit_shows_in_list_and_detail() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let id = add(&cx, "a");
    record(&cx, id, &limited());
    let want = (
        "agent_rate_limited".to_owned(),
        "Claude account rate limited (429)".to_owned(),
    );
    let listed = run(&cx, &req(None, None)).unwrap();
    assert_eq!(
        first_attention(listed.boxes.unwrap().first().unwrap()),
        want
    );
    let detail = run(&cx, &req(Some("a"), None)).unwrap();
    assert_eq!(first_attention(&detail.boxed.unwrap()), want);
}

#[test]
fn rate_limit_serializes_into_json() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let id = add(&cx, "a");
    record(&cx, id, &limited());
    let v = run(&cx, &req(Some("a"), None)).unwrap();
    let json = serde_json::to_value(v.boxed.unwrap()).unwrap();
    assert_eq!(
        json.pointer("/attention/0/kind").unwrap(),
        "agent_rate_limited"
    );
}

#[test]
fn recovery_clears_attention_and_the_json_key() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let id = add(&cx, "a");
    record(&cx, id, &limited());
    record(
        &cx,
        id,
        &Event::ProxyCredentialRecovered {
            rule: "claude".into(),
        },
    );
    let listed = run(&cx, &req(None, None)).unwrap();
    assert!(listed.boxes.unwrap().first().unwrap().attention.is_empty());
    let detail = run(&cx, &req(Some("a"), None)).unwrap().boxed.unwrap();
    assert!(detail.attention.is_empty());
    let json = serde_json::to_value(detail).unwrap();
    assert!(json.get("attention").is_none());
}
