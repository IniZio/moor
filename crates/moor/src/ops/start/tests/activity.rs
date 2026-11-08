use std::sync::Arc;
use std::time::Duration;

use moor_core::activity::{AgentState, Probe, Source, STARTUP_GRACE};
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::clock::Ts;
use moor_core::{BoxId, ShowReq};

use super::state::WORKING;
use super::state::{idle, working};
use super::{req, world, World};
use crate::ops::common::activity::{read, settle, PROBE_DEADLINE};
use crate::ops::show;
use crate::ops::start::run;
use crate::store::db::Conn;
use crate::store::db::DbExt as _;
use crate::store::BoxRow;
use crate::testing::agent;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use moor_core::domain::box_state::BoxLifecycle;
use moor_schema::tables::{Boxes, Launches, Runs};
use sea_query::{Expr, ExprTrait as _, Query};

const SESSION: &str = "s-1";

/// A second before the fake clock's now.
fn just_before(w: &World) -> Ts {
    Ts::from_millis(w.cx.clock().now().as_millis() - 1000)
}

/// An idle agent whose status changed just before now.
fn idle_now(w: &World) -> Probe {
    agent::idle(just_before(w))
}

/// A working agent whose status changed just before now.
fn working_now(w: &World) -> Probe {
    agent::working(just_before(w))
}

/// A started `demo` with a live session, probed as `probes` says; idle first when `idle`.
fn started(idle: bool, probes: impl FnOnce(&World) -> Probe) -> (World, BoxId) {
    let mut w = world(FakeDriver::new());
    let id = BoxId::new(run(&w.cx, &super::agentless_req("demo")).unwrap().boxed.id);
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
        .row(
            Query::select()
                .column(Boxes::IdleSince)
                .from(Boxes::Table)
                .and_where(Expr::col(Boxes::Id).eq(id.get())),
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn start_idles_a_working_box_whose_hook_reports_idle_then_wakes_it() {
    let (w, id) = started(false, idle_now);
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
    let (w, id) = started(true, working_now);
    assert!(idle(&row(&w, id)));
    let v = run(&w.cx, &req("demo")).unwrap();
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert!(working(&row(&w, id)));
}

#[test]
fn start_does_not_quiet_a_box_whose_agent_reports_no_status() {
    let (w, id) = started(false, |w| agent::read(None, just_before(w), 0));
    w.cx.clock().sleep(STARTUP_GRACE + Duration::from_secs(1));
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
    assert_eq!(activity.state, AgentState::Unknown);
    assert_eq!(activity.source, Source::Terminal);
    assert!(!reading.may_quiet);

    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(v.attached);
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert!(working(&row(&w, id)));
}

/// The newest session id of a Box.
fn newest_session(w: &World, id: BoxId) -> i64 {
    w.cx.store()
        .unwrap()
        .reader()
        .unwrap()
        .row(
            Query::select()
                .expr(Expr::col(Launches::Id).max())
                .from(Launches::Table)
                .and_where(Expr::col(Launches::Box).eq(id.get())),
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn a_no_agent_start_does_not_revive_a_box_whose_session_ended() {
    let (w, id) = started(false, |w| agent::read(None, just_before(w), 0));
    let store = w.cx.store().unwrap();
    while let Some(live) = store.live_session(id).unwrap() {
        store.end_session(live.id, Some(0), None).unwrap();
    }
    let ended = newest_session(&w, id);

    let v = run(&w.cx, &super::agentless_req("demo")).unwrap();
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
    let (w, id) = started(true, idle_now);
    w.cx.clock().sleep(Duration::from_mins(31));
    run(&w.cx, &super::agentless_req("other")).unwrap();
    assert_eq!(row(&w, id).lifecycle, BoxLifecycle::Asleep);
}

/// The kinds of the events of a Box.
fn kinds(w: &World, id: BoxId) -> Vec<String> {
    let events = w.cx.store().unwrap().events(id, 100).unwrap();
    events.into_iter().map(|e| e.kind).collect()
}

#[test]
fn restarting_an_idle_box_whose_agent_is_busy_ends_working_with_a_busy_event() {
    let (w, id) = started(true, working_now);
    let v = run(&w.cx, &req("demo")).unwrap();
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert!(working(&row(&w, id)));
    assert!(kinds(&w, id).contains(&"busy".to_owned()));
}

#[test]
fn restarting_an_idle_box_whose_agent_is_live_and_idle_ends_working_with_a_busy_event() {
    let (w, id) = started(true, idle_now);
    let v = run(&w.cx, &req("demo")).unwrap();
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert!(working(&row(&w, id)));
    assert!(kinds(&w, id).contains(&"busy".to_owned()));
}

#[test]
fn restarting_a_working_box_whose_agent_is_busy_leaves_it_working() {
    let (w, id) = started(false, working_now);
    let v = run(&w.cx, &req("demo")).unwrap();
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert!(working(&row(&w, id)));
    assert!(!kinds(&w, id).contains(&"idled".to_owned()));
}

#[test]
fn a_fresh_start_settles_another_box() {
    let (w, id) = started(false, idle_now);
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
    let engine = Arc::new(FakeEngine::new().probes(SESSION, idle_now(&w)));
    w.cx = w.cx.with_engine(engine);
    let raw = Conn::open_file(&w.cx.state_dir().unwrap().join("moor.db")).unwrap();
    raw.run(
        Query::insert()
            .into_table(Runs::Table)
            .columns([Runs::Box, Runs::Argv, Runs::Pid, Runs::StartedAt])
            .values_panic([bad.get().into(), "not json".into(), 1.into(), 1.into()]),
    )
    .unwrap();
    run(&w.cx, &req("other")).unwrap();
    assert!(idle(&row(&w, id)));
    let codes: Vec<_> = w.cx.take_warnings().into_iter().map(|x| x.code).collect();
    assert!(codes.contains(&"settle_failed".to_owned()), "{codes:?}");
}

fn registrations(w: &World) -> Vec<String> {
    w.surface
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("register_resume"))
        .collect()
}

#[test]
fn an_exited_agent_with_a_stored_id_registers_nothing() {
    let (w, id) = started(false, |_| agent::exited(Some(0)));
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
    let (w, id) = started(false, idle_now);
    let failed = Decision::new(id).set(Fields {
        setup_failed_at: Field::Set(w.cx.clock().now().as_millis()),
        ..Fields::default()
    });
    w.cx.store().unwrap().apply(failed).unwrap();
    run(&w.cx, &req("other")).unwrap();
    assert!(idle(&row(&w, id)));
    assert!(kinds(&w, id).contains(&"idled".to_owned()));
}
