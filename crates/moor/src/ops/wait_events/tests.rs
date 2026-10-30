//! Tests for `wait-events`.

use std::path::Path;
use std::sync::Arc;

use moor_core::activity::{HookKind, HookRead, HookRecord, PaneObs, PaneRead, PaneStatus, Probes};
use moor_core::controller::ServerFrame;
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::AcquireReq;
use moor_core::{BoxId, BoxName, ErrorKind, ProviderId, WaitEventsReq, WaitMode};

use super::run;
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

const SESSION: &str = "sess-1";

fn cx(dir: &Path) -> Cx {
    cx_with(dir, &Arc::new(FakeEngine::new()))
}

fn cx_with(dir: &Path, engine: &Arc<FakeEngine>) -> Cx {
    Cx::fake(
        dir,
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
    )
    .with_engine(engine.clone())
}

fn name(n: &str) -> BoxName {
    BoxName::parse_allow_bare(n).unwrap()
}

fn add(cx: &Cx, n: &str) -> BoxId {
    let nt = NewBox {
        name: name(n),
        repo: None,
    };
    cx.store()
        .unwrap()
        .apply(Decision::insert(nt))
        .unwrap()
        .box_id
}

fn attached(cx: &Cx, id: BoxId) {
    cx.store()
        .unwrap()
        .apply(Decision::new(id).event(ExtraEvent::Attached))
        .unwrap();
}

fn steered(cx: &Cx, id: BoxId) {
    let event = ExtraEvent::Steered {
        session: SESSION.to_owned(),
        message: "go".to_owned(),
        submit_retried: false,
        hook_before: None,
        new_turn: false,
    };
    cx.store()
        .unwrap()
        .apply(Decision::new(id).event(event))
        .unwrap();
}

fn req(boxes: &[&str], mode: WaitMode) -> WaitEventsReq {
    WaitEventsReq {
        boxes: boxes.iter().map(|b| name(b)).collect(),
        kinds: None,
        mode,
        since: None,
        timeout_secs: Some(2),
    }
}

fn boxes_of(frames: &[ServerFrame]) -> Vec<String> {
    frames
        .iter()
        .filter_map(|f| match f {
            ServerFrame::Event { r#box, .. } => r#box.clone(),
            _ => None,
        })
        .collect()
}

/// A Box named `n` whose machine is Ready.
fn ready(cx: &Cx, n: &str) -> BoxId {
    let id = add(cx, n);
    let got = cx
        .driver()
        .acquire(&AcquireReq {
            pool_machine: None,
            provider: ProviderId::new("p"),
            box_id: id,
            name: name(n),
            class: None,
            nonce: "abc123".into(),
            cache: moor_core::cache::CachePlan::none(),
        })
        .unwrap();
    let store = cx.store().unwrap();
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(ProviderId::new("p")),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .box_input(BoxInput::Acquired { handle: got.handle })
        .set(Fields {
            box_workdir: Field::Set("/work".into()),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    id
}

#[test]
fn any_returns_once_one_box_has_a_matching_event() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    add(&cx, "alpha");
    let beta = add(&cx, "beta");
    let mut r = req(&["alpha", "beta"], WaitMode::Any);
    r.since = Some(cx.store().unwrap().last_event_id().unwrap());
    attached(&cx, beta);
    let v = run(&cx, &r).unwrap();
    assert!(!v.timed_out);
    assert_eq!(v.waited_ms, 0);
    assert!(boxes_of(&v.events).iter().all(|b| b == "beta"));
    assert!(!v.events.is_empty());
}

#[test]
fn all_keeps_waiting_until_every_box_matched() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let alpha = add(&cx, "alpha");
    let beta = add(&cx, "beta");
    let mut r = req(&["alpha", "beta"], WaitMode::All);
    r.since = Some(0);
    r.kinds = Some(vec!["attached".to_owned()]);
    attached(&cx, alpha);
    let v = run(&cx, &r).unwrap();
    assert!(v.timed_out);
    assert_eq!(boxes_of(&v.events), ["alpha"]);
    attached(&cx, beta);
    let v = run(&cx, &r).unwrap();
    assert!(!v.timed_out);
    assert_eq!(boxes_of(&v.events), ["alpha", "beta"]);
}

#[test]
fn an_explicit_since_catches_an_earlier_event() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let id = add(&cx, "alpha");
    let before = cx.store().unwrap().last_event_id().unwrap();
    attached(&cx, id);
    let mut r = req(&["alpha"], WaitMode::Any);
    r.since = Some(before);
    let v = run(&cx, &r).unwrap();
    assert!(!v.timed_out);
    assert_eq!(v.cursor, cx.store().unwrap().last_event_id().unwrap());
}

#[test]
fn without_since_stored_events_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let id = add(&cx, "alpha");
    attached(&cx, id);
    let mut r = req(&["alpha"], WaitMode::Any);
    r.timeout_secs = Some(1);
    let v = run(&cx, &r).unwrap();
    assert!(v.timed_out);
    assert!(v.events.is_empty());
    assert_eq!(v.cursor, cx.store().unwrap().last_event_id().unwrap());
}

#[test]
fn the_kinds_filter_ignores_other_kinds() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let id = add(&cx, "alpha");
    attached(&cx, id);
    let mut r = req(&["alpha"], WaitMode::Any);
    r.since = Some(0);
    r.kinds = Some(vec!["steered".to_owned()]);
    r.timeout_secs = Some(1);
    assert!(run(&cx, &r).unwrap().timed_out);
    steered(&cx, id);
    let v = run(&cx, &r).unwrap();
    assert!(!v.timed_out);
    assert_eq!(v.events.len(), 1);
}

#[test]
fn an_unknown_box_fails_before_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    add(&cx, "alpha");
    let before = cx.clock().now();
    let e = run(&cx, &req(&["alpha", "ghost"], WaitMode::Any)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert!(e.error.to_string().contains("no Box ghost"), "{}", e.error);
    assert_eq!(cx.clock().now().as_millis(), before.as_millis());
}

#[test]
fn no_boxes_fails() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let e = run(&cx, &req(&[], WaitMode::Any)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
}

#[test]
fn the_op_settles_a_finished_steered_turn() {
    let dir = tempfile::tempdir().unwrap();
    let probes = Probes {
        hook: HookRead::Fresh(HookRecord {
            kind: HookKind::Idle,
            session_id: None,
            message: None,
            options: Vec::new(),
            doing: None,
            at: Ts::from_millis(i64::MAX / 2),
            turn: None,
        }),
        pane: PaneRead::Fresh(PaneObs {
            status: PaneStatus::Unknown,
            tail: String::new(),
            prompt: None,
            exit_marker: None,
        }),
    };
    let engine = Arc::new(FakeEngine::new().probes(SESSION, probes));
    let cx = cx_with(dir.path(), &engine);
    let id = ready(&cx, "alpha");
    cx.store()
        .unwrap()
        .begin_session(id, SESSION, None)
        .unwrap();
    steered(&cx, id);
    let mut r = req(&["alpha"], WaitMode::Any);
    r.kinds = Some(vec!["turn_done".to_owned()]);
    let v = run(&cx, &r).unwrap();
    assert!(!v.timed_out);
    assert_eq!(v.events.len(), 1);
}

#[test]
fn the_op_settles_a_steered_turn_on_a_box_whose_setup_failed() {
    let dir = tempfile::tempdir().unwrap();
    let probes = Probes {
        hook: HookRead::Fresh(HookRecord {
            kind: HookKind::Idle,
            session_id: None,
            message: None,
            options: Vec::new(),
            doing: None,
            at: Ts::from_millis(i64::MAX / 2),
            turn: None,
        }),
        pane: PaneRead::Fresh(PaneObs {
            status: PaneStatus::Unknown,
            tail: String::new(),
            prompt: None,
            exit_marker: None,
        }),
    };
    let engine = Arc::new(FakeEngine::new().probes(SESSION, probes));
    let cx = cx_with(dir.path(), &engine);
    let id = ready(&cx, "alpha");
    cx.store()
        .unwrap()
        .begin_session(id, SESSION, None)
        .unwrap();
    cx.store()
        .unwrap()
        .apply(Decision::new(id).set(Fields {
            setup_failed_at: Field::Set(1),
            ..Fields::default()
        }))
        .unwrap();
    steered(&cx, id);
    let mut r = req(&["alpha"], WaitMode::Any);
    r.kinds = Some(vec!["turn_done".to_owned()]);
    let v = run(&cx, &r).unwrap();
    assert!(!v.timed_out);
    assert_eq!(v.events.len(), 1);
}

#[test]
fn a_box_that_cannot_settle_warns_once_over_many_polls() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let id = ready(&cx, "alpha");
    cx.store()
        .unwrap()
        .begin_session(id, SESSION, None)
        .unwrap();
    let raw = rusqlite::Connection::open(cx.state_dir().unwrap().join("moor.db")).unwrap();
    raw.execute(
        "INSERT INTO runs (box, argv, pid, started_at) VALUES (?, 'not json', 1, 1)",
        [id.get()],
    )
    .unwrap();
    let mut r = req(&["alpha"], WaitMode::Any);
    r.timeout_secs = Some(2);
    assert!(run(&cx, &r).unwrap().timed_out);
    let warned = cx
        .take_warnings()
        .into_iter()
        .filter(|w| w.code == "settle_failed")
        .count();
    assert_eq!(warned, 1);
}

#[test]
fn a_closed_box_fails_before_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let id = add(&cx, "alpha");
    cx.store()
        .unwrap()
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Close)
                .session(SessionInput::End),
        )
        .unwrap();
    let before = cx.clock().now();
    let e = run(&cx, &req(&["alpha"], WaitMode::Any)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert!(
        e.error.to_string().contains("Box alpha is closed"),
        "{}",
        e.error
    );
    assert_eq!(cx.clock().now().as_millis(), before.as_millis());
}
