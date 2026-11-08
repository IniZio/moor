//! Tests for `inspect`.

use std::path::Path;
use std::sync::Arc;

use moor_core::activity::Probe;
use moor_core::domain::box_state::{BoxLifecycleInput, SetupOutcome};
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, Driver};
use moor_core::user::{Caller, Identity};
use moor_core::{BoxId, BoxName, InspectReq, ProviderId, Request};

use super::run;
use crate::cx::Cx;
use crate::ops::authz::authorize;
use crate::testing::agent;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::ScriptedProc;

fn cx(dir: &Path, driver: &Arc<FakeDriver>, engine: &Arc<FakeEngine>) -> Cx {
    Cx::fake(dir, driver.clone(), Arc::new(ScriptedProc::new())).with_engine(engine.clone())
}

fn req(name: &str, lines: Option<u32>, said: Option<u32>) -> InspectReq {
    InspectReq {
        target: BoxName::parse_allow_bare(name).unwrap().into(),
        lines,
        said,
    }
}

/// A working Box `a` with a live session `s-a`.
fn working(cx: &Cx, driver: &FakeDriver) -> BoxId {
    let name = BoxName::parse_allow_bare("a").unwrap();
    let id = cx
        .store()
        .unwrap()
        .apply(Decision::insert(NewBox {
            name: name.clone(),
            repo: None,
        }))
        .unwrap()
        .box_id;
    let got = driver
        .acquire(&AcquireReq {
            pool_machine: None,
            provider: ProviderId::new("p"),
            box_id: id,
            name,
            class: None,
            nonce: "abc123".into(),
            cache: moor_core::cache::CachePlan::none(),
        })
        .unwrap();
    let store = cx.store().unwrap();
    let admitted = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(ProviderId::new("p")),
            ..Fields::default()
        });
    store.apply(admitted).unwrap();
    let ready = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .box_input(BoxInput::Acquired { handle: got.handle })
        .set(Fields {
            box_workdir: Field::Set("/work".into()),
            ..Fields::default()
        });
    store.apply(ready).unwrap();
    store.begin_session(id, "s-a", None).unwrap();
    id
}

/// A read of a working agent whose screen shows `tail`.
fn pane(tail: &str) -> Probe {
    agent::on_screen(agent::working(Ts::from_millis(0)), tail)
}

fn numbered(n: usize) -> String {
    (1..=n).fold(String::new(), |t, i| t + &format!("line {i}\n"))
}

/// Box `a` over a pane of `n` numbered lines.
fn with_pane(n: usize) -> (tempfile::TempDir, Cx, BoxId) {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let setup = cx(dir.path(), &driver, &Arc::new(FakeEngine::new()));
    let id = working(&setup, &driver);
    let engine = Arc::new(FakeEngine::new().probes("s-a", pane(&numbered(n))));
    let cx = cx(dir.path(), &driver, &engine);
    (dir, cx, id)
}

fn put(cx: &Cx, id: BoxId, event: ExtraEvent) {
    cx.store()
        .unwrap()
        .apply(Decision::new(id).event(event))
        .unwrap();
}

#[test]
fn an_unknown_box_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = cx(dir.path(), &driver, &Arc::new(FakeEngine::new()));
    let e = run(&cx, &req("nope", None, None)).unwrap_err();
    assert!(e.error.to_string().contains("no Box nope"));
}

#[test]
fn the_pane_is_its_last_lines_with_its_status() {
    let (_dir, cx, _) = with_pane(300);
    let view = run(&cx, &req("a", Some(3), None)).unwrap();
    let pane = view.pane.unwrap();
    assert_eq!(pane.status, "live");
    assert_eq!(pane.tail, "line 298\nline 299\nline 300");
    assert_eq!(
        (view.box_name.as_str(), view.lifecycle.as_str()),
        ("a", "ready")
    );
    let default = run(&cx, &req("a", None, None)).unwrap().pane.unwrap();
    assert_eq!(default.tail.lines().count(), 40);
}

#[test]
fn lines_above_two_hundred_are_clamped() {
    let (_dir, cx, _) = with_pane(300);
    let pane = run(&cx, &req("a", Some(5000), None)).unwrap().pane.unwrap();
    assert_eq!(pane.tail.lines().count(), 200);
    assert_eq!(pane.tail.lines().next(), Some("line 101"));
}

#[test]
fn a_box_with_no_session_has_no_pane() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = cx(dir.path(), &driver, &Arc::new(FakeEngine::new()));
    let name = BoxName::parse_allow_bare("a").unwrap();
    cx.store()
        .unwrap()
        .apply(Decision::insert(NewBox { name, repo: None }))
        .unwrap();
    let view = run(&cx, &req("a", None, None)).unwrap();
    assert!(view.pane.is_none() && view.said.is_empty() && view.stuck.is_none());
}

#[test]
fn said_is_the_newest_messages_oldest_first() {
    let (_dir, cx, id) = with_pane(1);
    for n in 1..=8 {
        let text = format!("message {n}");
        put(&cx, id, ExtraEvent::AgentSaid { text });
        put(
            &cx,
            id,
            ExtraEvent::AgentDoing {
                text: "tool".into(),
            },
        );
    }
    let texts = |said: Option<u32>| -> Vec<String> {
        run(&cx, &req("a", None, said))
            .unwrap()
            .said
            .into_iter()
            .map(|s| s.text)
            .collect()
    };
    assert_eq!(texts(Some(3)), ["message 6", "message 7", "message 8"]);
    assert_eq!(texts(None).len(), 5);
    let all = run(&cx, &req("a", None, Some(500))).unwrap().said;
    assert_eq!(all.len(), 8);
    assert!(all.first().unwrap().at.ends_with('Z'));
}

#[test]
fn stuck_follows_the_events() {
    let (_dir, cx, id) = with_pane(1);
    assert!(run(&cx, &req("a", None, None)).unwrap().stuck.is_none());
    put(
        &cx,
        id,
        ExtraEvent::AgentUnreachable {
            attempts: 3,
            error: "timed out".into(),
        },
    );
    let stuck = run(&cx, &req("a", None, None)).unwrap().stuck.unwrap();
    assert_eq!(stuck.kind, "agent_unreachable");
    assert_eq!(stuck.detail, "unreachable after 3 tries: timed out");
    assert!(stuck.since.ends_with('Z'));
    put(
        &cx,
        id,
        ExtraEvent::AgentStalled {
            doing: None,
            quiet_secs: 700,
        },
    );
    assert_eq!(
        run(&cx, &req("a", None, None)).unwrap().stuck.unwrap().kind,
        "stalled"
    );
    put(&cx, id, ExtraEvent::AgentResumed { quiet_secs: 800 });
    assert!(run(&cx, &req("a", None, None)).unwrap().stuck.is_none());
}

#[test]
fn inspecting_changes_nothing() {
    let (_dir, cx, id) = with_pane(5);
    let store = cx.store().unwrap();
    let before = (store.box_row(id).unwrap(), store.events(id, 100).unwrap());
    run(&cx, &req("a", None, None)).unwrap();
    let after = (store.box_row(id).unwrap(), store.events(id, 100).unwrap());
    assert_eq!(before, after);
}

#[test]
fn another_users_box_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = cx(dir.path(), &driver, &Arc::new(FakeEngine::new()));
    let store = cx.store().unwrap();
    let (user, _) = store.enroll(&Identity::local(2000), None).unwrap();
    let name = BoxName::parse_allow_bare("a").unwrap();
    store
        .apply(Decision::insert(NewBox { name, repo: None }).requester(user))
        .unwrap();
    let request = Request::Inspect(req("a", None, None));
    assert!(authorize(&cx, &Ok(Caller::Local(1000)), None, &request).is_err());
    assert!(authorize(&cx, &Ok(Caller::Local(2000)), None, &request).is_ok());
}
