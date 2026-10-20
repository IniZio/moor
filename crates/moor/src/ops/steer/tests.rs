//! Tests for `steer`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_core::activity::{HookKind, HookRead, HookRecord, PaneRead, Probes};
use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, Driver};
use moor_core::seam::engine::{EngineError, Steer, SteerOutcome};
use moor_core::seam::proc::Tool;
use moor_core::{BoxId, BoxName, ErrorKind, ProviderId, SteerReq};

use std::time::Duration;

use super::run;
use crate::cx::Cx;
use crate::ops::common::boxlock::BoxLock;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;

struct Fx {
    _dir: tempfile::TempDir,
    cx: Cx,
    engine: Arc<FakeEngine>,
    driver: Arc<FakeDriver>,
    surface: Arc<FakeSurface>,
    id: BoxId,
}

/// Task `t` with a ready Box, an Idle state and a live session `agent-1` in pane `p-1`.
fn fx_with(engine: FakeEngine, session: bool) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let engine = Arc::new(engine);
    let surface = Arc::new(FakeSurface::new(PathBuf::from("/s")));
    let proc = Arc::new(ScriptedProc::new());
    proc.on(
        Tool::Git,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        Reply::Ok("/home/me/proj/.git\n".into()),
    );
    let cx = Cx::fake(
        dir.path(),
        Path::new("/home/me/proj"),
        driver.clone(),
        surface.clone(),
        proc,
    )
    .with_engine(engine.clone());
    let id = ready_task(&cx, &driver);
    if session {
        cx.store()
            .unwrap()
            .begin_session(id, "agent-1", Some("p-1"))
            .unwrap();
    }
    Fx {
        _dir: dir,
        cx,
        engine,
        driver,
        surface,
        id,
    }
}

/// Task `t` with a ready Box.
fn ready_task(cx: &Cx, driver: &FakeDriver) -> BoxId {
    let store = cx.store().unwrap();
    let name = BoxName::parse_allow_bare("t").unwrap();
    let id = store
        .apply(Decision::insert(NewBox {
            name: name.clone(),
            repo: "r".into(),
        }))
        .unwrap()
        .box_id;
    let provider = ProviderId::new("p");
    let fields = Fields {
        box_provider: Field::Set(provider.clone()),
        worktree: Field::Set("/home/me/wt/t".to_owned()),
        ..Fields::default()
    };
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(fields);
    store.apply(d).unwrap();
    let got = driver
        .acquire(&AcquireReq {
            provider,
            box_id: id,
            name,
            class: None,
            nonce: "n".to_owned(),
            worktree: None,
        })
        .unwrap();
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .box_input(BoxInput::Acquired {
            handle: got.handle.clone(),
        })
        .set(Fields {
            box_handle: Field::Set(got.handle),
            box_workdir: Field::Set(got.workdir.as_str().to_owned()),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    id
}

fn fx() -> Fx {
    fx_with(FakeEngine::new(), true)
}

/// Probes that read a blocked agent waiting on `question`.
fn blocked(question: &str) -> Probes {
    Probes {
        hook: HookRead::Fresh(HookRecord {
            kind: HookKind::Blocked,
            session_id: None,
            message: Some(question.to_owned()),
            at: Ts::from_millis(0),
        }),
        pane: PaneRead::Absent,
    }
}

/// Probes that read an agent working.
fn working() -> Probes {
    Probes {
        hook: HookRead::Fresh(HookRecord {
            kind: HookKind::Busy,
            session_id: None,
            message: None,
            at: Ts::from_millis(0),
        }),
        pane: PaneRead::Absent,
    }
}

fn text(t: &str) -> SteerReq {
    SteerReq {
        box_name: BoxName::parse_allow_bare("t").unwrap(),
        text: Some(t.to_owned()),
        menu: None,
    }
}

fn menu(n: u32) -> SteerReq {
    SteerReq {
        box_name: BoxName::parse_allow_bare("t").unwrap(),
        text: None,
        menu: Some(n),
    }
}

fn live(f: &Fx) -> bool {
    f.cx.store().unwrap().live_session(f.id).unwrap().is_some()
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
fn an_unknown_task_is_refused() {
    let f = fx();
    let req = SteerReq {
        box_name: BoxName::parse_allow_bare("other").unwrap(),
        ..text("hi")
    };
    let err = run(&f.cx, &req).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert!(err.error.to_string().contains("no open task other"));
}

#[test]
fn a_task_without_a_live_session_is_refused_naming_start_goal() {
    let f = fx_with(FakeEngine::new(), false);
    let err = run(&f.cx, &text("hi")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(err
        .error
        .to_string()
        .contains("`moor start t --goal \"...\"`"));
    assert!(f.engine.steers().is_empty());
}

#[test]
fn text_goes_to_the_agent_and_is_recorded() {
    let f = fx_with(FakeEngine::new().probes("agent-1", working()), true);
    let v = run(&f.cx, &text("do x")).unwrap();
    assert_eq!((v.box_name.as_str(), v.sent.as_str()), ("t", "do x"));
    assert!(!v.answered_menu);
    assert_eq!(v.steer, steered_id(&f));
    assert_eq!(
        f.engine.steers(),
        vec![("agent-1".to_owned(), Steer::Text("do x".into()))]
    );
    assert_eq!(
        steered(&f),
        vec![serde_json::json!({
            "session": "agent-1",
            "message": "do x",
            "submit_retried": false,
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
fn an_asleep_task_wakes_before_the_send() {
    let f = fx();
    let store = f.cx.store().unwrap();
    let quiet = Decision::new(f.id).session(SessionInput::AgentQuiet);
    store.apply(quiet).unwrap();
    let d = Decision::new(f.id)
        .lifecycle(BoxLifecycleInput::Sleep { auto: false })
        .box_input(BoxInput::Sleep);
    store.apply(d).unwrap();
    run(&f.cx, &text("hi")).unwrap();
    let row = store.box_row(f.id).unwrap().unwrap();
    assert_ne!(row.lifecycle, BoxLifecycle::Asleep);
    assert_eq!(row.boxed.unwrap().state, BoxState::Ready);
    assert!(f.driver.calls().iter().any(|c| c.starts_with("wake ")));
    assert_eq!(f.engine.steers().len(), 1);
}

#[test]
fn a_gone_pane_stops_the_session_and_refuses() {
    let engine = FakeEngine::new().steer_result(Err(EngineError::PaneGone));
    let f = fx_with(engine, true);
    let err = run(&f.cx, &text("hi")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(
        err.error.to_string(),
        "the agent's pane is gone; relaunch with `moor start t --goal \"...\"`"
    );
    assert!(!live(&f));
    assert!(steered(&f).is_empty());
}

#[test]
fn an_exit_steer_on_a_gone_pane_succeeds_and_ends_the_session() {
    let engine = FakeEngine::new().steer_result(Err(EngineError::PaneGone));
    let f = fx_with(engine, true);
    let v = run(&f.cx, &text(" /exit ")).unwrap();
    assert_eq!(v.sent, " /exit ");
    assert!(!v.answered_menu);
    assert_eq!(v.steer, steered_id(&f));
    assert!(!live(&f));
    let warned = f.cx.take_warnings();
    assert_eq!(
        warned.first().map(|w| w.code.as_str()),
        Some("agent_exited")
    );
}

#[test]
fn an_exited_agent_names_its_code() {
    let exited = Probes {
        hook: HookRead::Absent,
        pane: PaneRead::Fresh(moor_core::activity::PaneObs {
            status: moor_core::activity::PaneStatus::Idle,
            tail: String::new(),
            prompt: None,
            exit_marker: Some(3),
        }),
    };
    let engine = FakeEngine::new()
        .probes("agent-1", exited)
        .steer_result(Err(EngineError::PaneGone));
    let f = fx_with(engine, true);
    let err = run(&f.cx, &text("hi")).unwrap_err();
    assert_eq!(
        err.error.to_string(),
        "agent exited 3; relaunch with `moor start t --goal \"...\"`"
    );
    let exit: Option<i32> =
        f.cx.store()
            .unwrap()
            .reader()
            .unwrap()
            .query_row(
                "SELECT exit FROM launches WHERE box = ?",
                [f.id.get()],
                |r| r.get(0),
            )
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
    assert!(msg.contains("--menu N"), "{msg}");
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

fn hold_lock(f: &Fx) -> BoxLock {
    BoxLock::acquire(
        f.cx.clock(),
        f.cx.state_dir().unwrap(),
        f.id,
        "run",
        Duration::ZERO,
    )
    .unwrap()
}

fn put_to_sleep(f: &Fx) {
    let store = f.cx.store().unwrap();
    let quiet = Decision::new(f.id).session(SessionInput::AgentQuiet);
    store.apply(quiet).unwrap();
    let d = Decision::new(f.id)
        .lifecycle(BoxLifecycleInput::Sleep { auto: false })
        .box_input(BoxInput::Sleep);
    store.apply(d).unwrap();
}

#[test]
fn steering_during_a_run_is_sent_while_the_box_lock_is_held() {
    let f = fx();
    let _held = hold_lock(&f);
    run(&f.cx, &text("hi")).unwrap();
    assert_eq!(f.engine.steers().len(), 1);
}

#[test]
fn an_asleep_task_under_a_held_lock_is_refused_and_nothing_is_sent() {
    let f = fx();
    put_to_sleep(&f);
    let _held = hold_lock(&f);
    let err = run(&f.cx, &text("hi")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(err.error.to_string().contains("busy"), "{}", err.error);
    assert!(f.engine.steers().is_empty());
}

#[test]
fn an_asleep_task_without_a_live_session_is_refused_without_a_wake() {
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
fn a_session_without_an_id_records_its_pane_in_the_event() {
    let f = fx_with(FakeEngine::new(), false);
    f.cx.store()
        .unwrap()
        .begin_session(f.id, "", Some("p-9"))
        .unwrap();
    run(&f.cx, &text("hi")).unwrap();
    assert_eq!(
        steered(&f)
            .first()
            .and_then(|p| p.get("session"))
            .and_then(serde_json::Value::as_str),
        Some("p-9")
    );
}

#[test]
fn an_exited_agent_has_its_transcript_saved_before_the_session_ends() {
    let exited = Probes {
        hook: HookRead::Absent,
        pane: PaneRead::Fresh(moor_core::activity::PaneObs {
            status: moor_core::activity::PaneStatus::Idle,
            tail: String::new(),
            prompt: None,
            exit_marker: Some(3),
        }),
    };
    let engine = FakeEngine::new()
        .probes("agent-1", exited)
        .transcript_text("abc")
        .steer_result(Err(EngineError::PaneGone));
    let f = fx_with(engine, true);
    let store = f.cx.store().unwrap();
    let current = store.live_session(f.id).unwrap().unwrap();
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
    let digest: Option<String> = store
        .reader()
        .unwrap()
        .query_row(
            "SELECT transcript_digest FROM launches WHERE box = ?",
            [f.id.get()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        digest.as_deref(),
        Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
    );
}

mod cold_boot;
