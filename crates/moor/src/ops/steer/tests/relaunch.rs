//! A text steer to a Box whose agent exited waits while the inbox pass relaunches the agent
//! once, then sends; a menu choice relaunches it at once.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use moor_core::domain::hands::HandsInc;
use std::time::Duration;

use moor_core::activity::Probe;
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields};
use moor_core::domain::inbox::QueueCause;
use moor_core::domain::session_state::SessionState;
use moor_core::seam::engine::{EngineError, SessionRef, Steer};
use moor_core::view::steer::Delivery;
use moor_core::{BoxHandle, BoxPath};

use super::queue::{pass, reports_idle};
use super::{blocked, fx_with, menu, steered, text, Fx};
use crate::ops::start::revive_for_steer;
use crate::ops::steer::run;
use crate::ops::steer::{relaunch, Gone};
use crate::testing::agent;
use crate::testing::engine::FakeEngine;

/// A read of an agent that exited 3.
fn exited() -> Probe {
    agent::exited(Some(3))
}

/// The session the relaunch starts, `agent-2`.
fn relaunched_session() -> SessionRef {
    SessionRef {
        id: "agent-2".to_owned(),
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
    }
}

/// Box `t` with an agent that has an agent session to resume.
fn resumable(engine: FakeEngine) -> Fx {
    let f = fx_with(engine.launch_result(Ok(relaunched_session())), true);
    let set = Fields {
        agent_session_id: Field::Set("stored-1".into()),
        ..Fields::default()
    };
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).set(set))
        .unwrap();
    f
}

/// The relaunched agent reports its agent session, as its hook does.
fn resumes(f: &Fx) {
    let reported = Fields {
        agent_session_id: Field::Set("stored-1".into()),
        ..Fields::default()
    };
    let store = f.cx.store().unwrap();
    store.apply(Decision::new(f.id).set(reported)).unwrap();
}

fn relaunches(f: &Fx) -> Vec<serde_json::Value> {
    let rows = f.cx.store().unwrap().events(f.id, 100).unwrap();
    rows.into_iter()
        .filter(|e| e.kind == "relaunched")
        .map(|e| e.payload)
        .collect()
}

fn session_state(f: &Fx) -> SessionState {
    f.cx.store()
        .unwrap()
        .box_row(f.id)
        .unwrap()
        .unwrap()
        .session
}

/// Steers `t`, which waits for the relaunch, and checks it says so.
fn queued(f: &Fx, t: &str, why: QueueCause) {
    let v = run(&f.cx, &text(t)).unwrap();
    assert_eq!((v.delivery, v.steer), (Delivery::Queued(why), None));
}

#[test]
fn a_gone_agent_is_relaunched_by_the_pass_with_its_stored_session_then_steered() {
    let engine = FakeEngine::new();
    let f = resumable(engine.probes("agent-1", agent::gone()));
    queued(&f, "hi", QueueCause::Relaunching);
    assert!(
        f.engine.launches().is_empty(),
        "the steer itself relaunches nothing"
    );
    reports_idle(&f, "agent-2");
    pass(&f);
    let launches = f.engine.launches();
    assert_eq!(launches.len(), 1);
    assert_eq!(launches[0].resume.as_deref(), Some("stored-1"));
    let causes: Vec<_> = relaunches(&f).iter().map(|e| e["cause"].clone()).collect();
    assert_eq!(causes, [serde_json::json!("steer")]);
    let sends = f.engine.steers();
    assert_eq!(sends.len(), 1, "nothing goes to the gone agent");
    assert_eq!(sends[0].0, "agent-2");
    assert_eq!(steered(&f).len(), 1);
}

#[test]
fn an_agent_gone_again_soon_after_its_relaunch_is_not_relaunched_and_its_steer_waits() {
    let engine = FakeEngine::new();
    let f = resumable(engine.probes("agent-1", agent::gone()));
    queued(&f, "hi", QueueCause::Relaunching);
    f.engine.set_probes("agent-2", agent::gone());
    pass(&f);
    resumes(&f);
    f.cx.take_warnings();
    pass(&f);
    let warned = f.cx.take_warnings();
    assert!(
        warned
            .iter()
            .any(|w| w.message.contains("again within 60s")),
        "{warned:?}"
    );
    assert_eq!(f.engine.launches().len(), 1, "relaunched once only");
    assert_eq!(session_state(&f), SessionState::Ended);
    assert!(steered(&f).is_empty());
    assert_eq!(f.cx.store().unwrap().unsent(f.id).unwrap().len(), 1);
    pass(&f);
    assert!(
        f.cx.take_warnings().is_empty(),
        "backs off within the guard"
    );
    resumes(&f);
    f.cx.clock().sleep(Duration::from_secs(61));
    pass(&f);
    assert_eq!(f.engine.launches().len(), 2, "tried again after it");
}

#[test]
fn a_gone_agent_a_minute_after_its_relaunch_is_relaunched_again() {
    let engine = FakeEngine::new();
    let f = resumable(engine.probes("agent-1", agent::gone()));
    queued(&f, "one", QueueCause::Relaunching);
    reports_idle(&f, "agent-2");
    pass(&f);
    resumes(&f);
    f.engine.set_probes("agent-2", agent::gone());
    f.cx.clock().sleep(Duration::from_secs(61));
    queued(&f, "two", QueueCause::Relaunching);
    pass(&f);
    assert_eq!(f.engine.launches().len(), 2);
}

#[test]
fn an_answer_to_an_exited_agent_relaunches_it_and_picks_the_choice() {
    let engine = FakeEngine::new()
        .probes("agent-1", exited())
        .probes("agent-2", blocked("Allow edit?"));
    let f = resumable(engine);
    run(&f.cx, &menu(1)).unwrap();
    assert_eq!(f.engine.launches().len(), 1);
    let sends = f.engine.steers();
    assert_eq!(sends.len(), 1, "nothing goes to the exited agent");
    assert_eq!(sends[0], ("agent-2".to_owned(), Steer::Menu(1)));
}

#[test]
fn a_steer_queued_before_another_relaunch_goes_to_the_new_agent_first() {
    let engine = FakeEngine::new();
    let f = resumable(engine.probes("agent-1", agent::gone()));
    let store = f.cx.store().unwrap();
    let old = store.live_session(f.id).unwrap();
    queued(&f, "one", QueueCause::Relaunching);
    let gone = Gone::new("t", old, None);
    relaunch::once(&f.cx, store, (f.id, "t"), gone).unwrap();
    assert_eq!(session_state(&f), SessionState::Working);
    reports_idle(&f, "agent-2");
    let v = run(&f.cx, &text("two")).unwrap();
    assert_eq!(v.delivery, Delivery::Delivered);
    assert_eq!(f.engine.launches().len(), 1);
    let sent: Vec<_> = steered(&f).iter().map(|e| e["message"].clone()).collect();
    assert_eq!(sent, ["one", "two"]);
}

#[test]
fn an_asleep_box_gets_no_new_session_from_a_steer_relaunch() {
    let f = resumable(FakeEngine::new());
    let store = f.cx.store().unwrap();
    assert!(store.end_current_session(f.id, "exited", None).unwrap());
    put_to_sleep_ended(&f);
    let before = store.box_sessions(f.id).unwrap().len();
    let view = revive_for_steer(&f.cx, store, f.id).unwrap();
    assert!(!view.launched);
    assert_eq!(store.box_sessions(f.id).unwrap().len(), before);
}

/// Puts the machine and Box to sleep after the Session ended.
fn put_to_sleep_ended(f: &Fx) {
    let d = Decision::new(f.id)
        .lifecycle(moor_core::domain::box_state::BoxLifecycleInput::Sleep {
            auto: false,
            reason: None,
        })
        .hands_input(HandsInc::FIRST, moor_core::domain::hands::HandsInput::Sleep);
    f.cx.store().unwrap().apply(d).unwrap();
}

/// Records `n` automatic relaunches of the agent of Box `f`.
fn record_relaunches(f: &Fx, n: usize) {
    for _ in 0..n {
        let event = ExtraEvent::Relaunched {
            prompt: None,
            cause: Some("wake".to_owned()),
        };
        f.cx.store()
            .unwrap()
            .apply(Decision::new(f.id).event(event))
            .unwrap();
    }
}

fn kinds(f: &Fx, kind: &str) -> Vec<serde_json::Value> {
    let rows = f.cx.store().unwrap().events(f.id, 100).unwrap();
    rows.into_iter()
        .filter(|e| e.kind == kind)
        .map(|e| e.payload)
        .collect()
}

#[test]
fn a_steer_after_three_automatic_relaunches_within_ten_minutes_waits_as_capped() {
    let engine = FakeEngine::new();
    let f = resumable(engine.probes("agent-1", agent::gone()));
    record_relaunches(&f, 3);
    f.cx.clock().sleep(Duration::from_secs(90));
    queued(&f, "hi", QueueCause::RelaunchCapped);
    f.cx.take_warnings();
    pass(&f);
    pass(&f);
    assert!(
        f.cx.take_warnings().is_empty(),
        "the pass holds a capped Box"
    );
    assert!(f.engine.launches().is_empty(), "no agent was launched");
    assert_eq!(
        kinds(&f, "relaunch_capped"),
        [serde_json::json!({"count": 3, "window_secs": 600})],
        "recorded once, not on every pass"
    );
    assert_eq!(f.cx.store().unwrap().unsent(f.id).unwrap().len(), 1);
}

#[test]
fn relaunches_older_than_ten_minutes_do_not_count() {
    let engine = FakeEngine::new();
    let f = resumable(engine.probes("agent-1", agent::gone()));
    record_relaunches(&f, 3);
    f.cx.clock().sleep(Duration::from_mins(11));
    queued(&f, "hi", QueueCause::Relaunching);
    pass(&f);
    assert_eq!(f.engine.launches().len(), 1);
    assert!(kinds(&f, "relaunch_capped").is_empty());
}

#[test]
fn a_steer_refused_while_the_agent_still_runs_is_sent_again_and_never_relaunches() {
    let engine = FakeEngine::new().steer_result(Err(EngineError::AgentGone));
    let f = resumable(engine);
    let v = run(&f.cx, &text("hi")).unwrap();
    assert_eq!(v.sent, "hi");
    assert!(f.engine.launches().is_empty(), "no second agent");
    assert!(relaunches(&f).is_empty());
    assert_eq!(f.engine.steers().len(), 2, "one failed send, one retry");
    assert_eq!(steered(&f).len(), 1);
    assert_ne!(session_state(&f), SessionState::Ended);
    let ended = kinds(&f, "session_ended");
    assert!(ended.is_empty(), "{ended:?}");
}

#[test]
fn a_steer_refused_twice_while_the_agent_still_runs_is_sent_a_third_time() {
    let engine = FakeEngine::new()
        .steer_result(Err(EngineError::AgentGone))
        .steer_result(Err(EngineError::AgentGone));
    let f = resumable(engine);
    let v = run(&f.cx, &text("hi")).unwrap();
    assert_eq!(v.sent, "hi");
    assert!(f.engine.launches().is_empty(), "no second agent");
    assert_ne!(session_state(&f), SessionState::Ended);
}

#[test]
fn a_steer_refused_every_time_while_the_agent_still_runs_fails_without_naming_a_relaunch() {
    let engine = FakeEngine::new()
        .steer_result(Err(EngineError::AgentGone))
        .steer_result(Err(EngineError::AgentGone))
        .steer_result(Err(EngineError::AgentGone));
    let f = resumable(engine);
    let err = run(&f.cx, &text("hi")).unwrap_err();
    let message = err.error.to_string();
    assert!(!message.contains("of its relaunch"), "{message}");
    assert!(f.engine.launches().is_empty(), "no second agent");
    assert_ne!(session_state(&f), SessionState::Ended);
}
