//! Tests for the agent's newest message and the Reports in the detail of `show`.

use std::sync::Arc;

use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::ShowReq;

use super::super::run;
use super::{add, cx, req};
use crate::testing::driver::FakeDriver;

/// Request for Box `a` with the agent.
fn with_agent(events: Option<u32>) -> ShowReq {
    ShowReq {
        agent: true,
        ..req(Some("a"), events)
    }
}

/// Appends `event` to Box `id`'s events through the store.
fn put(cx: &crate::cx::Cx, id: moor_core::BoxId, event: ExtraEvent) {
    cx.store()
        .unwrap()
        .apply(Decision::new(id).event(event))
        .unwrap();
}

#[test]
fn detail_returns_the_newest_message_older_than_the_event_window() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let id = add(&cx, "a");
    for text in ["first", "second"] {
        let text = text.to_owned();
        put(&cx, id, ExtraEvent::AgentSaid { text });
    }
    for n in 0..8 {
        let text = format!("step {n}");
        put(&cx, id, ExtraEvent::AgentDoing { text });
    }
    let shown = run(&cx, &with_agent(Some(3))).unwrap();
    let said = shown.boxed.unwrap().said.unwrap();
    assert_eq!(said.text, "second");
    assert!(said.at.ends_with('Z'), "{}", said.at);
    assert!(shown
        .events
        .unwrap()
        .iter()
        .all(|e| e.kind == "agent_doing"));
    let unasked = run(&cx, &req(Some("a"), None)).unwrap();
    assert!(unasked.boxed.unwrap().said.is_none());
}

#[test]
fn reports_are_returned_latest_version_per_kind_when_asked() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let id = add(&cx, "a");
    let store = cx.store().unwrap();
    let out = dir.path().join("reports");
    for (kind, body) in [("pr", "v1"), ("pr", "v2"), ("qa", "ok")] {
        store.add_report(id, kind, body, &out).unwrap();
    }
    let asked = ShowReq {
        reports: true,
        ..req(Some("a"), None)
    };
    let shown = run(&cx, &asked).unwrap().reports.unwrap();
    let rows: Vec<_> = shown.iter().map(|r| (r.kind.as_str(), r.version)).collect();
    assert_eq!(rows, [("pr", 2), ("qa", 1)]);
    assert!(run(&cx, &req(Some("a"), None)).unwrap().reports.is_none());
}
