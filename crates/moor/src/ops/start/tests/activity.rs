use std::sync::Arc;
use std::time::Duration;

use moor_core::activity::{
    AgentState, HookKind, HookRead, HookRecord, PaneObs, PaneRead, PaneStatus, Probes, Source,
    STARTUP_GRACE,
};
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::clock::Ts;
use moor_core::{BoxId, BoxName, ShowReq};

use super::state::WORKING;
use super::state::{idle, working};
use super::{req, world, World};
use crate::ops::common::activity::{read, settle, PROBE_DEADLINE};
use crate::ops::show;
use crate::ops::start::run;
use crate::store::BoxRow;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use moor_core::domain::box_state::BoxLifecycle;

const SESSION: &str = "s-1";

/// Probes with `hook` and a pane that reports `pane`.
fn probes(w: &World, hook: Option<HookKind>, pane: PaneStatus) -> Probes {
    let at = Ts::from_millis(w.cx.clock().now().as_millis() - 1000);
    Probes {
        hook: hook.map_or(HookRead::Absent, |kind| {
            HookRead::Fresh(HookRecord {
                kind,
                session_id: None,
                message: None,
                options: Vec::new(),
                at,
            })
        }),
        pane: PaneRead::Fresh(PaneObs {
            status: pane,
            tail: String::new(),
            prompt: None,
            exit_marker: None,
        }),
    }
}

/// A started `demo` with a live session, probed as `probes` says; idle first when `idle`.
fn started(idle: bool, probes: impl FnOnce(&World) -> Probes) -> (World, BoxId) {
    let mut w = world(FakeDriver::new());
    let id = BoxId::new(run(&w.cx, &req("demo")).unwrap().boxed.id);
    let store = w.cx.store().unwrap();
    store.begin_session(id, SESSION, None).unwrap();
    if idle {
        store
            .apply(Decision::new(id).session(SessionInput::AgentQuiet))
            .unwrap();
    }
    let engine = Arc::new(FakeEngine::new().probes(SESSION, probes(&w)));
    w.cx = w.cx.with_engine(engine);
    (w, id)
}

fn row(w: &World, id: BoxId) -> BoxRow {
    w.cx.store().unwrap().box_row(id).unwrap().unwrap()
}

/// The `idle_since` column of a Box.
fn idle_since(w: &World, id: BoxId) -> Option<i64> {
    w.cx.store()
        .unwrap()
        .reader()
        .unwrap()
        .query_row(
            "SELECT idle_since FROM boxes WHERE id = ?",
            [id.get()],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn start_idles_a_working_box_whose_hook_reports_idle_then_wakes_it() {
    let (w, id) = started(false, |w| {
        probes(w, Some(HookKind::Idle), PaneStatus::Unknown)
    });
    assert!(working(&row(&w, id)));
    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(v.attached);
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert!(working(&row(&w, id)));
    let k = kinds(&w, id);
    let idled = k.iter().position(|x| x == "idled").unwrap();
    assert!(k.iter().rposition(|x| x == "busy").unwrap() > idled);
    assert!(idle_since(&w, id).is_none());
}

#[test]
fn start_wakes_an_idle_box_whose_hook_reports_working() {
    let (w, id) = started(true, |w| {
        probes(w, Some(HookKind::Busy), PaneStatus::Unknown)
    });
    assert!(idle(&row(&w, id)));
    let v = run(&w.cx, &req("demo")).unwrap();
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert!(working(&row(&w, id)));
}

/// A started `demo` whose session is older than the startup grace, with no hook report
/// and a fresh pane read of `pane`.
fn aged_pane_only(pane: PaneStatus) -> (World, BoxId) {
    let (w, id) = started(false, |w| probes(w, None, pane));
    w.cx.clock().sleep(STARTUP_GRACE + Duration::from_secs(1));
    (w, id)
}

/// Reads `pane` with no hook report, returning the reading's state, then starts the Box.
/// Asserts the pane-only reading never quiets the Box.
fn start_on_pane_only(pane: PaneStatus) -> AgentState {
    let (w, id) = aged_pane_only(pane);
    let store = w.cx.store().unwrap();
    let reading = read(
        &w.cx,
        store,
        &row(&w, id),
        w.cx.clock().now(),
        PROBE_DEADLINE,
    )
    .unwrap();
    let activity = reading.activity.expect("a live session has a reading");
    assert!(matches!(activity.source, Source::Herdr | Source::Pane));
    assert!(!reading.may_quiet);

    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(v.attached);
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert!(working(&row(&w, id)));
    activity.state
}

#[test]
fn start_does_not_quiet_a_box_on_a_pane_only_idle_report() {
    assert_eq!(start_on_pane_only(PaneStatus::Idle), AgentState::Idle);
}

#[test]
fn start_does_not_quiet_a_box_on_a_pane_only_blocked_report() {
    assert!(matches!(
        start_on_pane_only(PaneStatus::Blocked),
        AgentState::NeedsInput { .. }
    ));
}

/// The newest session id of a Box.
fn newest_session(w: &World, id: BoxId) -> i64 {
    w.cx.store()
        .unwrap()
        .reader()
        .unwrap()
        .query_row(
            "SELECT MAX(id) FROM launches WHERE box = ?",
            [id.get()],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn start_does_not_revive_a_box_whose_session_ended() {
    let (w, id) = started(false, |w| probes(w, None, PaneStatus::Unknown));
    let store = w.cx.store().unwrap();
    while let Some(live) = store.live_session(id).unwrap() {
        store.end_session(live.id, Some(0), None).unwrap();
    }
    let ended = newest_session(&w, id);

    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(v.attached);

    assert!(store.live_session(id).unwrap().is_none());
    assert_eq!(newest_session(&w, id), ended);

    let list = ShowReq {
        target: None,
        events: None,
        agent: false,
        reports: false,
        artifacts: false,
        pane: false,
    };
    show::run(&w.cx, &list).unwrap();
    assert!(idle(&row(&w, id)));
}

#[test]
fn start_sleeps_an_idle_box_past_the_threshold() {
    let (w, id) = started(true, |w| {
        probes(w, Some(HookKind::Idle), PaneStatus::Unknown)
    });
    w.cx.clock().sleep(Duration::from_mins(31));
    let v = run(&w.cx, &req("demo")).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Asleep);
    assert_eq!(row(&w, id).lifecycle, BoxLifecycle::Asleep);
}

/// The kinds of the events of a Box.
fn kinds(w: &World, id: BoxId) -> Vec<String> {
    let events = w.cx.store().unwrap().events(id, 100).unwrap();
    events.into_iter().map(|e| e.kind).collect()
}

#[test]
fn restarting_an_idle_box_whose_agent_is_busy_ends_working_with_a_busy_event() {
    let (w, id) = started(true, |w| {
        probes(w, Some(HookKind::Busy), PaneStatus::Unknown)
    });
    let v = run(&w.cx, &req("demo")).unwrap();
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert!(working(&row(&w, id)));
    assert!(kinds(&w, id).contains(&"busy".to_owned()));
}

#[test]
fn restarting_an_idle_box_whose_agent_is_live_and_idle_ends_working_with_a_busy_event() {
    let (w, id) = started(true, |w| {
        probes(w, Some(HookKind::Idle), PaneStatus::Unknown)
    });
    let v = run(&w.cx, &req("demo")).unwrap();
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert!(working(&row(&w, id)));
    assert!(kinds(&w, id).contains(&"busy".to_owned()));
}

#[test]
fn restarting_a_working_box_whose_agent_is_busy_leaves_it_working() {
    let (w, id) = started(false, |w| {
        probes(w, Some(HookKind::Busy), PaneStatus::Unknown)
    });
    let v = run(&w.cx, &req("demo")).unwrap();
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert!(working(&row(&w, id)));
    assert!(!kinds(&w, id).contains(&"idled".to_owned()));
}

#[test]
fn a_fresh_start_settles_another_box() {
    let (w, id) = started(false, |w| {
        probes(w, Some(HookKind::Idle), PaneStatus::Unknown)
    });
    run(&w.cx, &req("other")).unwrap();
    assert!(idle(&row(&w, id)));
    assert!(kinds(&w, id).contains(&"idled".to_owned()));
}

#[test]
fn a_box_that_cannot_be_settled_warns_and_the_others_still_settle() {
    let mut w = world(FakeDriver::new());
    let bad = BoxId::new(run(&w.cx, &req("bad")).unwrap().boxed.id);
    let id = BoxId::new(run(&w.cx, &req("demo")).unwrap().boxed.id);
    w.cx.store()
        .unwrap()
        .begin_session(id, SESSION, None)
        .unwrap();
    let engine = Arc::new(FakeEngine::new().probes(
        SESSION,
        probes(&w, Some(HookKind::Idle), PaneStatus::Unknown),
    ));
    w.cx = w.cx.with_engine(engine);
    let raw = rusqlite::Connection::open(w.cx.state_dir().unwrap().join("moor.db")).unwrap();
    raw.execute(
        "INSERT INTO runs (box, argv, pid, started_at) VALUES (?, 'not json', 1, 1)",
        [bad.get()],
    )
    .unwrap();
    run(&w.cx, &req("other")).unwrap();
    assert!(idle(&row(&w, id)));
    let codes: Vec<_> = w.cx.take_warnings().into_iter().map(|x| x.code).collect();
    assert!(codes.contains(&"settle_failed".to_owned()), "{codes:?}");
}

/// A started `demo` whose session sits in pane `p-9` and whose hook reports session `agent-1`.
fn hooked() -> (World, BoxId) {
    let mut w = world(FakeDriver::new());
    let id = BoxId::new(run(&w.cx, &req("demo")).unwrap().boxed.id);
    w.cx.store()
        .unwrap()
        .begin_session(id, SESSION, Some("p-9"))
        .unwrap();
    let mut p = probes(&w, Some(HookKind::Busy), PaneStatus::Working);
    p.hook = HookRead::Fresh(HookRecord {
        kind: HookKind::Busy,
        session_id: Some("agent-1".into()),
        message: None,
        options: Vec::new(),
        at: Ts::from_millis(w.cx.clock().now().as_millis() - 1000),
    });
    w.cx =
        w.cx.with_engine(Arc::new(FakeEngine::new().probes(SESSION, p)));
    (w, id)
}

fn registrations(w: &World) -> Vec<String> {
    w.surface
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("register_resume"))
        .collect()
}

fn show_box() -> ShowReq {
    ShowReq {
        target: Some(BoxName::parse_allow_bare("demo").unwrap().into()),
        events: None,
        agent: false,
        reports: false,
        artifacts: false,
        pane: false,
    }
}

#[test]
fn the_first_hook_session_id_is_registered_once() {
    let (w, _id) = hooked();
    w.surface.have_pane("p-9");
    run(&w.cx, &req("demo")).unwrap();
    run(&w.cx, &req("demo")).unwrap();
    assert_eq!(registrations(&w), ["register_resume p-9 claude agent-1"]);
}

#[test]
fn an_id_a_show_stored_first_is_still_registered_by_the_next_start() {
    let (w, id) = hooked();
    w.surface.have_pane("p-9");
    show::run(&w.cx, &show_box()).unwrap();
    assert_eq!(row(&w, id).agent_session_id.as_deref(), Some("agent-1"));
    assert!(registrations(&w).is_empty());
    run(&w.cx, &req("demo")).unwrap();
    assert_eq!(registrations(&w), ["register_resume p-9 claude agent-1"]);
}

#[test]
fn a_failed_registration_is_tried_again_by_a_later_start() {
    let (w, _id) = hooked();
    run(&w.cx, &req("demo")).unwrap();
    assert!(w
        .cx
        .take_warnings()
        .iter()
        .any(|x| x.code == "resume_not_registered"));
    w.surface.have_pane("p-9");
    run(&w.cx, &req("demo")).unwrap();
    run(&w.cx, &req("demo")).unwrap();
    assert_eq!(registrations(&w).len(), 2);
    assert_eq!(
        registrations(&w).last().map(String::as_str),
        Some("register_resume p-9 claude agent-1")
    );
}

#[test]
fn a_stored_id_is_registered_when_the_hook_is_absent() {
    let (w, id) = started(false, |w| probes(w, None, PaneStatus::Working));
    let set = Fields {
        agent_session_id: Field::Set("agent-1".into()),
        ..Fields::default()
    };
    w.cx.store()
        .unwrap()
        .apply(Decision::new(id).set(set))
        .unwrap();
    w.cx.store()
        .unwrap()
        .begin_session(id, SESSION, Some("p-9"))
        .unwrap();
    w.surface.have_pane("p-9");
    run(&w.cx, &req("demo")).unwrap();
    assert_eq!(registrations(&w), ["register_resume p-9 claude agent-1"]);
}

#[test]
fn an_exited_pane_with_a_stored_id_registers_nothing() {
    let (w, id) = started(false, |w| {
        let mut p = probes(w, None, PaneStatus::Working);
        if let PaneRead::Fresh(obs) = &mut p.pane {
            obs.exit_marker = Some(0);
        }
        p
    });
    let store = w.cx.store().unwrap();
    store.begin_session(id, SESSION, Some("p-9")).unwrap();
    let set = Fields {
        agent_session_id: Field::Set("agent-1".into()),
        ..Fields::default()
    };
    store.apply(Decision::new(id).set(set)).unwrap();
    w.surface.have_pane("p-9");
    let r = row(&w, id);
    let reading = read(&w.cx, store, &r, w.cx.clock().now(), PROBE_DEADLINE).unwrap();
    assert_eq!(reading.pane.as_str(), "exited");
    settle(&w.cx, store, &r, &reading).unwrap();
    assert!(registrations(&w).is_empty());
    let codes: Vec<_> = w.cx.take_warnings().into_iter().map(|x| x.code).collect();
    assert!(
        !codes.contains(&"resume_not_registered".to_owned()),
        "{codes:?}"
    );
}

#[test]
fn a_box_whose_setup_failed_still_settles_its_live_agent() {
    let (w, id) = started(false, |w| {
        probes(w, Some(HookKind::Idle), PaneStatus::Unknown)
    });
    let failed = Decision::new(id).set(Fields {
        setup_failed_at: Field::Set(w.cx.clock().now().as_millis()),
        ..Fields::default()
    });
    w.cx.store().unwrap().apply(failed).unwrap();
    run(&w.cx, &req("other")).unwrap();
    assert!(idle(&row(&w, id)));
    assert!(kinds(&w, id).contains(&"idled".to_owned()));
}
