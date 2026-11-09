//! Tests for `steer`.

use crate::testing::db::column_where;
use moor_schema::tables::Launches;
use std::sync::Arc;

use moor_core::activity::Probe;
use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::decision::{Decision, Field, HandsFields};
use moor_core::domain::hands::{HandsInc, HandsInput, HandsState};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::agent_adapter::FirstRunPrompt;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, Driver};
use moor_core::seam::engine::{EngineError, Steer, SteerOutcome};
use moor_core::seam::proc::Tool;
use moor_core::{BoxId, BoxName, ErrorDetail, ErrorKind, ProviderId, SteerReq};

use super::run;
use crate::cx::owners::{Cmd, Held, RunHold};
use crate::cx::Cx;
use crate::store::db::DbExt as _;
use crate::testing::agent;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::{Reply, ScriptedProc};

struct Fx {
    dir: tempfile::TempDir,
    cx: Cx,
    engine: Arc<FakeEngine>,
    driver: Arc<FakeDriver>,
    id: BoxId,
    backoff: crate::ops::InboxBackoff,
}

/// Box `t` with a ready machine, an Idle state and a live session `agent-1`.
fn fx_with(engine: FakeEngine, session: bool) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let engine = Arc::new(engine);
    let proc = Arc::new(ScriptedProc::new());
    proc.on(
        Tool::Git,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        Reply::Ok("/home/me/proj/.git\n".into()),
    );
    let cx = Cx::fake(dir.path(), driver.clone(), proc).with_engine(engine.clone());
    let id = ready_box(&cx, &driver);
    if session {
        cx.store().unwrap().begin_launch(id, "agent-1").unwrap();
    }
    Fx {
        dir,
        cx,
        engine,
        driver,
        id,
        backoff: crate::ops::InboxBackoff::default(),
    }
}

/// Box `t` with a ready machine.
fn ready_box(cx: &Cx, driver: &FakeDriver) -> BoxId {
    let store = cx.store().unwrap();
    let name = BoxName::parse_allow_bare("t").unwrap();
    let id = store
        .apply(crate::testing::cx::new_box(name.clone()))
        .unwrap()
        .box_id;
    let provider = ProviderId::new("p");
    let fields = HandsFields {
        provider: Field::Set(provider.clone()),
        ..HandsFields::default()
    };
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .hands_input(HandsInc::FIRST, HandsInput::Requested)
        .hands(HandsInc::FIRST, fields);
    store.apply(d).unwrap();
    let got = driver
        .acquire(&AcquireReq {
            pool_machine: None,
            provider,
            box_id: id,
            name,
            class: None,
            nonce: "n".to_owned(),
            cache: moor_core::cache::CachePlan::none(),
        })
        .unwrap();
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .hands_input(
            HandsInc::FIRST,
            HandsInput::Acquired {
                handle: got.handle.clone(),
            },
        )
        .hands(
            HandsInc::FIRST,
            HandsFields {
                handle: Field::Set(got.handle),
                workdir: Field::Set(got.workdir.as_str().to_owned()),
                ..HandsFields::default()
            },
        );
    store.apply(d).unwrap();
    id
}

fn fx() -> Fx {
    fx_with(FakeEngine::new(), true)
}

/// A read of an agent blocked on a permission, asking `question`.
fn blocked(question: &str) -> Probe {
    agent::permission(question, Ts::from_millis(0))
}

/// A read of a working agent.
fn working() -> Probe {
    agent::working(Ts::from_millis(0))
}

fn text(t: &str) -> SteerReq {
    SteerReq {
        target: BoxName::parse_allow_bare("t").unwrap().into(),
        text: Some(t.to_owned()),
        menu: None,
        interrupt: false,
    }
}

fn menu(n: u32) -> SteerReq {
    SteerReq {
        target: BoxName::parse_allow_bare("t").unwrap().into(),
        text: None,
        menu: Some(n),
        interrupt: false,
    }
}

fn interrupt() -> SteerReq {
    SteerReq {
        target: BoxName::parse_allow_bare("t").unwrap().into(),
        text: None,
        menu: None,
        interrupt: true,
    }
}

fn live(f: &Fx) -> bool {
    f.cx.store().unwrap().live_launch(f.id).unwrap().is_some()
}

fn steered(f: &Fx) -> Vec<serde_json::Value> {
    let rows = f.cx.store().unwrap().events(f.id, 100).unwrap();
    rows.into_iter()
        .filter(|e| e.kind == "steered")
        .map(|e| e.payload)
        .collect()
}

fn steered_id(f: &Fx) -> i64 {
    let rows = f.cx.store().unwrap().events(f.id, 100).unwrap();
    let ids: Vec<i64> = rows
        .into_iter()
        .filter(|e| e.kind == "steered")
        .map(|e| e.id)
        .collect();
    assert_eq!(ids.len(), 1);
    ids.first().copied().unwrap()
}

#[test]
fn an_unknown_box_is_refused() {
    let f = fx();
    let req = SteerReq {
        target: BoxName::parse_allow_bare("other").unwrap().into(),
        ..text("hi")
    };
    let err = run(&f.cx, &req).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert!(err.error.to_string().contains("no open Box other"));
}

#[test]
fn a_box_without_a_live_session_is_refused_naming_start_prompt() {
    let f = fx_with(FakeEngine::new(), false);
    let err = run(&f.cx, &text("hi")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(err
        .error
        .to_string()
        .contains("`moor start t --prompt \"...\"`"));
    assert!(f.engine.steers().is_empty());
}

#[test]
fn text_goes_to_the_agent_and_is_recorded() {
    let f = fx_with(FakeEngine::new().probes("agent-1", working()), true);
    let v = run(&f.cx, &text("do x")).unwrap();
    assert_eq!((v.box_name.as_str(), v.sent.as_str()), ("t", "do x"));
    assert!(!v.answered_menu);
    assert_eq!(v.steer, Some(steered_id(&f)));
    assert_eq!(
        f.engine.steers(),
        vec![("agent-1".to_owned(), Steer::Text("do x".into()))]
    );
    let row = f.cx.store().unwrap().sent(f.id).unwrap();
    assert_eq!(
        steered(&f),
        vec![serde_json::json!({
            "session": "agent-1",
            "message": "do x",
            "submit_retried": false,
            "turn_before": 1,
            "inbox": row.first().unwrap().id.get(),
        })]
    );
    assert!(f.cx.take_warnings().is_empty());
}

#[test]
fn a_retried_submit_is_recorded() {
    let engine = FakeEngine::new().steer_result(Ok(SteerOutcome {
        submit_retried: true,
        submit_checked: true,
    }));
    let f = fx_with(engine, true);
    run(&f.cx, &text("do x")).unwrap();
    assert_eq!(
        steered(&f)
            .first()
            .and_then(|p| p.get("submit_retried"))
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
}

#[test]
fn an_unchecked_submit_warns() {
    let engine = FakeEngine::new().steer_result(Ok(SteerOutcome {
        submit_retried: false,
        submit_checked: false,
    }));
    let f = fx_with(engine, true);
    run(&f.cx, &text("do x")).unwrap();
    let warned = f.cx.take_warnings();
    let msgs: Vec<_> = warned.iter().map(|w| w.message.as_str()).collect();
    assert_eq!(msgs, ["could not confirm the message was submitted"]);
}

#[test]
fn an_asleep_box_wakes_before_the_send() {
    let f = fx();
    let store = f.cx.store().unwrap();
    put_to_sleep(&f);
    run(&f.cx, &text("hi")).unwrap();
    queue::reports_idle(&f, "agent-1");
    queue::pass(&f);
    let row = store.box_row(f.id).unwrap().unwrap();
    assert_ne!(row.lifecycle, BoxLifecycle::Asleep);
    assert_eq!(row.hands.unwrap().state, HandsState::Ready);
    assert!(f.driver.calls().iter().any(|c| c.starts_with("wake ")));
    assert_eq!(f.engine.steers().len(), 1);
}

#[test]
fn a_gone_pane_stops_the_session_and_refuses() {
    let engine = FakeEngine::new().probes("agent-1", agent::gone());
    let f = fx_with(engine, true);
    let err = run(&f.cx, &text("hi")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(
        err.error.to_string(),
        "the agent's pane is gone; relaunch with `moor start t --prompt \"...\"`"
    );
    assert_eq!(err.detail.as_deref(), Some(&ErrorDetail::AgentGone));
    assert!(!live(&f));
    assert!(steered(&f).is_empty());
}

#[test]
fn an_exit_steer_on_a_gone_pane_succeeds_and_ends_the_session() {
    let engine = FakeEngine::new().steer_result(Err(EngineError::AgentGone));
    let f = fx_with(engine, true);
    let v = run(&f.cx, &text(" /exit ")).unwrap();
    assert_eq!(v.sent, " /exit ");
    assert!(!v.answered_menu);
    assert_eq!(v.steer, Some(steered_id(&f)));
    assert!(!live(&f));
    let warned = f.cx.take_warnings();
    assert_eq!(
        warned.first().map(|w| w.code.as_str()),
        Some("agent_exited")
    );
}

#[test]
fn an_exited_agent_names_its_code() {
    let exited = agent::exited(Some(3));
    let engine = FakeEngine::new().probes("agent-1", exited);
    let f = fx_with(engine, true);
    let err = run(&f.cx, &text("hi")).unwrap_err();
    assert_eq!(
        err.error.to_string(),
        "agent exited 3; relaunch with `moor start t --prompt \"...\"`"
    );
    assert_eq!(err.detail.as_deref(), Some(&ErrorDetail::AgentGone));
    let q = column_where(Launches::Table, Launches::Exit, Launches::Box, f.id.get());
    let exit: Option<i32> =
        f.cx.store()
            .unwrap()
            .reader()
            .unwrap()
            .row(q, |r| r.get(0))
            .unwrap();
    assert_eq!(exit, Some(3));
}

#[test]
fn text_to_a_blocked_agent_is_refused_with_its_prompt() {
    let f = fx_with(
        FakeEngine::new().probes("agent-1", blocked("Allow edit?")),
        true,
    );
    let err = run(&f.cx, &text("go on")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    let msg = err.error.to_string();
    assert!(msg.contains("waiting for: Allow edit?"), "{msg}");
    assert!(!msg.contains("Workspace"), "{msg}");
    assert!(f.engine.steers().is_empty());
}

fn which_color() -> Probe {
    agent::asking("Which color?", &["red", "blue"], Ts::from_millis(0))
}

#[test]
fn an_option_named_as_text_answers_the_question() {
    let engine = FakeEngine::new()
        .probes("agent-1", working())
        .then_probes("agent-1", which_color());
    let f = fx_with(engine, true);
    let v = run(&f.cx, &text("  Blue ")).unwrap();
    assert!(v.answered_menu);
    assert_eq!(
        f.engine.steers(),
        vec![("agent-1".to_owned(), Steer::Menu(2))]
    );
}

#[test]
fn an_option_number_as_text_answers_the_question() {
    let engine = FakeEngine::new()
        .probes("agent-1", working())
        .then_probes("agent-1", which_color());
    let f = fx_with(engine, true);
    run(&f.cx, &text("1")).unwrap();
    assert_eq!(
        f.engine.steers(),
        vec![("agent-1".to_owned(), Steer::Menu(1))]
    );
}

#[test]
fn other_text_to_a_question_is_refused_with_its_options() {
    let f = fx_with(FakeEngine::new().probes("agent-1", which_color()), true);
    for t in ["green", "3"] {
        let err = run(&f.cx, &text(t)).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Refused);
        let msg = err.error.to_string();
        assert!(msg.contains("waiting for: Which color?"), "{msg}");
        assert!(msg.contains("1. red") && msg.contains("2. blue"), "{msg}");
        assert!(msg.contains("send one of"), "{msg}");
        assert!(!msg.contains("Workspace"), "{msg}");
    }
    assert!(f.engine.steers().is_empty());
}

#[test]
fn a_digit_to_a_blocked_agent_picks_the_menu_entry_and_probes_again() {
    let engine = FakeEngine::new()
        .probes("agent-1", working())
        .then_probes("agent-1", blocked("Allow edit?"));
    let f = fx_with(engine, true);
    let v = run(&f.cx, &text("2")).unwrap();
    assert!(v.answered_menu);
    assert_eq!(v.sent, "2");
    assert_eq!(
        f.engine.steers(),
        vec![("agent-1".to_owned(), Steer::Menu(2))]
    );
    assert_eq!(f.engine.probe_count("agent-1"), 2);
    assert!(f.cx.take_warnings().is_empty());
}

#[test]
fn a_menu_choice_that_leaves_the_agent_blocked_warns() {
    let f = fx_with(
        FakeEngine::new().probes("agent-1", blocked("Allow edit?")),
        true,
    );
    let v = run(&f.cx, &menu(1)).unwrap();
    assert!(!v.answered_menu);
    assert_eq!(f.engine.probe_count("agent-1"), 2);
    assert_eq!(f.cx.take_warnings().len(), 1);
}

#[test]
fn a_first_run_screen_refuses_every_steer_and_sends_nothing() {
    let at_launch = agent::on_screen(
        agent::read(None, Ts::from_millis(0), 0),
        include_str!("../../adapters/box_agent/claude/fixtures/folder_trust.txt"),
    );
    let engine = FakeEngine::new()
        .prompt("trust this folder", FirstRunPrompt::FolderTrust)
        .probes("agent-1", at_launch);
    let f = fx_with(engine, true);
    for req in [menu(1), menu(2), text("1"), text("yes")] {
        let err = run(&f.cx, &req).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Refused);
        assert!(err.error.to_string().contains("moor bug"), "{}", err.error);
    }
    assert!(f.engine.steers().is_empty());
}

#[test]
fn menu_on_an_agent_that_is_not_blocked_is_refused() {
    let f = fx_with(FakeEngine::new().probes("agent-1", working()), true);
    let err = run(&f.cx, &menu(1)).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(f.engine.steers().is_empty());
}

#[test]
fn a_digit_to_an_agent_that_is_not_blocked_is_text() {
    let f = fx_with(FakeEngine::new().probes("agent-1", working()), true);
    run(&f.cx, &text("2")).unwrap();
    assert_eq!(
        f.engine.steers(),
        vec![("agent-1".to_owned(), Steer::Text("2".into()))]
    );
}

fn hold_lock(f: &Fx) -> Held {
    f.cx.owners_arc().hold(f.id, Cmd::Run)
}

/// A run in flight on the Box, past the turn that started it.
fn run_in_flight(f: &Fx) -> RunHold<'_> {
    f.cx.turn(f.id, Cmd::Run).unwrap().begin_run()
}

fn put_to_sleep(f: &Fx) {
    let store = f.cx.store().unwrap();
    let quiet = Decision::new(f.id).session(SessionInput::AgentQuiet);
    store.apply(quiet).unwrap();
    let d = Decision::new(f.id)
        .lifecycle(BoxLifecycleInput::sleep(false))
        .hands_input(HandsInc::FIRST, HandsInput::Sleep);
    store.apply(d).unwrap();
}

#[test]
fn steering_during_a_run_is_sent_while_the_box_lock_is_held() {
    let f = fx();
    let _running = run_in_flight(&f);
    run(&f.cx, &text("hi")).unwrap();
    assert_eq!(f.engine.steers().len(), 1);
}

#[test]
fn an_asleep_box_under_a_held_lock_is_queued_and_nothing_is_sent() {
    let f = fx();
    put_to_sleep(&f);
    let _held = hold_lock(&f);
    let v = run(&f.cx, &text("hi")).unwrap();
    assert_eq!(
        v.delivery,
        moor_core::view::steer::Delivery::Queued(moor_core::domain::inbox::QueueCause::Asleep)
    );
    assert!(f.engine.steers().is_empty());
}

#[test]
fn an_asleep_box_without_a_live_session_is_refused_without_a_wake() {
    let f = fx_with(FakeEngine::new(), false);
    put_to_sleep(&f);
    let err = run(&f.cx, &text("hi")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(f.driver.calls().iter().all(|c| !c.starts_with("wake ")));
}

#[test]
fn a_two_digit_answer_to_a_blocked_agent_is_refused() {
    let f = fx_with(
        FakeEngine::new().probes("agent-1", blocked("Allow edit?")),
        true,
    );
    let err = run(&f.cx, &text("12")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(f.engine.steers().is_empty());
}

#[test]
fn menu_zero_is_a_usage_error() {
    let f = fx_with(
        FakeEngine::new().probes("agent-1", blocked("Allow edit?")),
        true,
    );
    let err = run(&f.cx, &menu(0)).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
    assert!(f.engine.steers().is_empty());
}

#[test]
fn an_exited_agent_has_its_transcript_saved_before_the_session_ends() {
    let exited = agent::exited(Some(3));
    let engine = FakeEngine::new()
        .probes("agent-1", exited)
        .transcript_text("abc");
    let f = fx_with(engine, true);
    let store = f.cx.store().unwrap();
    let current = store.live_launch(f.id).unwrap().unwrap();
    store.set_registered_session(current.id, "agent-1").unwrap();
    run(&f.cx, &text("hi")).unwrap_err();
    assert_eq!(f.engine.transcripts(), ["agent-1"]);
    let store = f.cx.store().unwrap();
    let events = store.events(f.id, 100).unwrap();
    let saved: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "transcript_saved")
        .collect();
    assert_eq!(saved.len(), 1);
    assert_eq!(
        saved.first().and_then(|e| e.payload.get("trigger")),
        Some(&serde_json::json!("exit"))
    );
    assert!(!live(&f));
    let q = column_where(
        Launches::Table,
        Launches::TranscriptDigest,
        Launches::Box,
        f.id.get(),
    );
    let digest: Option<String> = store.reader().unwrap().row(q, |r| r.get(0)).unwrap();
    assert_eq!(
        digest.as_deref(),
        Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
    );
}

mod arrival;
mod cold_boot;
mod inbox;
mod interrupt;
mod queue;
mod relaunch;
mod stale_launch;
mod target;
mod turn_end;
mod turns;
