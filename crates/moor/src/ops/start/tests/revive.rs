use moor_core::domain::decision::{Decision, Field, Fields};
use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::ExtraEvent;
use moor_core::seam::engine::{AgentFate, EngineError, Steer};
use moor_core::seam::proxy_host::ProxyStatus;
use moor_core::seam::surface::SurfaceError;
use moor_core::BoxId;

use super::dead_agent::{ca_stopped, recorded_failure, CA};
use super::relaunch::{gone, obs, prompt_req, session, set_agent_session, started};
use super::{world, World};
use crate::ops::common::wake::wake_and_revive;
use crate::ops::start::{revive_after_wake, run};
use crate::store::db::DbExt as _;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proxy_host::{FakeProxyHost, ProxyHostCall};
use moor_schema::tables::Launches;
use sea_query::{Expr, ExprTrait as _, Order, Query};

fn revive(w: &World, id: BoxId) {
    revive_after_wake(&w.cx, w.cx.store().unwrap(), id).unwrap();
}

fn relaunched(w: &World, id: BoxId) -> Vec<serde_json::Value> {
    w.cx.store()
        .unwrap()
        .events(id, 100)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "relaunched")
        .map(|e| e.payload)
        .collect()
}

#[test]
fn a_gone_pane_after_wake_resumes_the_stored_session() {
    let (w, engine, id) = started();
    set_agent_session(&w, id, "agent-1");
    gone(&w);
    revive(&w, id);
    let all = engine.launches();
    assert_eq!(all.len(), 2);
    assert_eq!(
        all.get(1).and_then(|l| l.resume.as_deref()),
        Some("agent-1")
    );
    let events = relaunched(&w, id);
    assert_eq!(events.len(), 1);
    assert_eq!(
        events.first().and_then(|e| e.get("cause")),
        Some(&"wake".into())
    );
}

#[test]
fn an_exited_pane_after_wake_relaunches_and_keeps_the_old_exit() {
    let (w, engine, id) = started();
    set_agent_session(&w, id, "agent-1");
    w.surface.push_pane_obs("p-1", Ok(obs(Some(3))));
    revive(&w, id);
    assert_eq!(engine.launches().len(), 2);
    let exit: Option<i32> =
        w.cx.store()
            .unwrap()
            .reader()
            .unwrap()
            .row(
                Query::select()
                    .column(Launches::Exit)
                    .from(Launches::Table)
                    .and_where(Expr::col(Launches::Box).eq(id.get()))
                    .order_by(Launches::Id, Order::Asc)
                    .limit(1),
                |r| r.get(0),
            )
            .unwrap();
    assert_eq!(exit, Some(3));
    assert_eq!(relaunched(&w, id).len(), 1);
}

#[test]
fn a_live_pane_after_wake_is_left_alone() {
    let (w, engine, id) = started();
    set_agent_session(&w, id, "agent-1");
    w.surface.push_pane_obs("p-1", Ok(obs(None)));
    revive(&w, id);
    assert_eq!(engine.launches().len(), 1);
    assert!(relaunched(&w, id).is_empty());
}

#[test]
fn an_unreadable_pane_after_wake_counts_as_live() {
    let (w, engine, id) = started();
    w.surface.push_pane_obs("p-1", Err(SurfaceError::Timeout));
    revive(&w, id);
    assert_eq!(engine.launches().len(), 1);
    assert!(relaunched(&w, id).is_empty());
}

#[test]
fn nothing_to_resume_and_no_prompt_launches_nothing() {
    let (w, engine, id) = started();
    let clear = Fields {
        prompt: Field::Clear,
        ..Fields::default()
    };
    w.cx.store()
        .unwrap()
        .apply(Decision::new(id).set(clear))
        .unwrap();
    gone(&w);
    revive(&w, id);
    assert_eq!(engine.launches().len(), 1);
    assert!(relaunched(&w, id).is_empty());
}

const UP: ProxyStatus = ProxyStatus::Ready { box_port: 40123 };

fn elapsed_ms(w: &World, since: i64) -> i64 {
    w.cx.clock().now().as_millis() - since
}

fn status_calls(host: &FakeProxyHost) -> usize {
    host.calls()
        .iter()
        .filter(|c| matches!(c, ProxyHostCall::Status(_)))
        .count()
}

#[test]
fn the_relaunch_waits_for_the_tunnel_to_come_up() {
    let (mut w, engine, id) = started();
    set_agent_session(&w, id, "agent-1");
    gone(&w);
    let host =
        FakeProxyHost::new().with_status_script([ProxyStatus::Starting, ProxyStatus::Starting, UP]);
    w.cx =
        w.cx.with_proxy_host(Arc::new(host.clone()))
            .with_prompt_window(Duration::ZERO);
    let before = w.cx.clock().now().as_millis();
    revive(&w, id);
    assert_eq!(engine.launches().len(), 2);
    assert_eq!(status_calls(&host), 3);
    assert_eq!(elapsed_ms(&w, before), 2000);
}

#[test]
fn the_relaunch_waits_out_a_recent_tunnel_restart() {
    let (mut w, engine, id) = started();
    set_agent_session(&w, id, "agent-1");
    gone(&w);
    let host = FakeProxyHost::new().with_status(UP);
    w.cx =
        w.cx.with_proxy_host(Arc::new(host))
            .with_prompt_window(Duration::ZERO);
    let restart = ExtraEvent::TunnelRestarted {
        box_port: 40123,
        reason: "tunnel exited".into(),
    };
    w.cx.store()
        .unwrap()
        .apply(Decision::new(id).event(restart))
        .unwrap();
    w.cx.clock().sleep(Duration::from_secs(2));
    let before = w.cx.clock().now().as_millis();
    revive(&w, id);
    assert_eq!(engine.launches().len(), 2);
    assert_eq!(elapsed_ms(&w, before), 3000);
}

#[test]
fn the_relaunch_goes_ahead_when_the_tunnel_never_settles() {
    let (mut w, engine, id) = started();
    set_agent_session(&w, id, "agent-1");
    gone(&w);
    let host = FakeProxyHost::new().with_status(ProxyStatus::Starting);
    w.cx =
        w.cx.with_proxy_host(Arc::new(host))
            .with_prompt_window(Duration::ZERO);
    let before = w.cx.clock().now().as_millis();
    revive(&w, id);
    assert_eq!(engine.launches().len(), 2);
    assert_eq!(elapsed_ms(&w, before), 60_000);
}

#[test]
fn a_relaunch_after_wake_makes_no_box_call() {
    let (w, engine, id) = started();
    set_agent_session(&w, id, "agent-1");
    gone(&w);
    let before = w.driver.calls();
    revive(&w, id);
    assert_eq!(engine.launches().len(), 2);
    assert_eq!(w.driver.calls(), before);
}

#[test]
fn a_relaunch_after_wake_stopped_by_an_unwritable_proxy_ca_records_the_ca_message() {
    let (w, engine, id) = started();
    set_agent_session(&w, id, "agent-1");
    gone(&w);
    w.surface.push_pane_obs("p-2", Ok(ca_stopped()));
    revive(&w, id);
    assert_eq!(engine.launches().len(), 2);
    assert_eq!(recorded_failure(&w, id).as_deref(), Some(CA));
    assert!(w.cx.store().unwrap().live_session(id).unwrap().is_none());
}

/// As [`started`], with the Box answering `fates` when asked after its agent.
fn answering(fates: Vec<Result<AgentFate, EngineError>>) -> (World, Arc<FakeEngine>, BoxId) {
    let engine = Arc::new(
        FakeEngine::new()
            .launch_result(Ok(session("p-1")))
            .launch_result(Ok(session("p-2")))
            .fates(fates),
    );
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_engine(engine.clone());
    let first = run(&w.cx, &prompt_req("ship it")).unwrap();
    let id = BoxId::new(first.boxed.id);
    set_agent_session(&w, id, "agent-1");
    (w, engine, id)
}

fn silent() -> Vec<Result<AgentFate, EngineError>> {
    let failed = EngineError::Failed {
        step: "fate",
        message: "ssh: connection refused".to_owned(),
    };
    vec![Err(failed); 4]
}

#[test]
fn a_live_agent_after_wake_is_reattached_without_a_launch() {
    let (w, engine, id) = answering(vec![Ok(AgentFate::Running)]);
    gone(&w);
    revive(&w, id);
    assert_eq!(
        engine.launches().len(),
        1,
        "a launch kills the holder's terminal"
    );
    assert_eq!(engine.reattached().len(), 1);
    assert!(relaunched(&w, id).is_empty());
    let live = w.cx.store().unwrap().live_session(id).unwrap().unwrap();
    assert_eq!(live.pane.as_deref(), Some("reattached-1"));
}

#[test]
fn a_dead_agent_after_wake_relaunches_only_once_the_box_says_so() {
    let (w, engine, id) = answering(vec![Ok(AgentFate::Ended { exit: Some(9) })]);
    gone(&w);
    revive(&w, id);
    assert_eq!(engine.fates_left(), 0, "the Box was asked first");
    assert_eq!(engine.launches().len(), 2);
    assert!(engine.reattached().is_empty());
}

#[test]
fn a_box_that_does_not_answer_after_wake_launches_nothing_yet() {
    let (w, engine, id) = answering(silent());
    gone(&w);
    revive(&w, id);
    assert_eq!(engine.launches().len(), 1);
    assert!(w.cx.store().unwrap().live_session(id).unwrap().is_some());
}

/// Suspends `id` mid-turn for `reason`, ten seconds ago, then wakes it with its pane gone.
fn woken_after_suspend(w: &World, id: BoxId, reason: &str) {
    let store = w.cx.store().unwrap();
    let suspend = BoxLifecycleInput::Suspend {
        reason: reason.to_owned(),
    };
    store
        .apply(
            Decision::new(id)
                .lifecycle(suspend)
                .box_input(BoxInput::Sleep),
        )
        .unwrap();
    let at = w.cx.clock().now().as_millis() / 1000;
    store.set_resume_first(id, Some(at)).unwrap();
    w.cx.clock().sleep(Duration::from_secs(10));
    gone(w);
    let row = store.box_row(id).unwrap().unwrap();
    wake_and_revive(&w.cx, store, &row).unwrap();
}

fn notes(engine: &FakeEngine) -> Vec<String> {
    engine
        .steers()
        .into_iter()
        .filter_map(|(_, s)| match s {
            Steer::Text(t) => Some(t),
            _ => None,
        })
        .collect()
}

const CRITICAL: &str = "last-resort suspend: memory critical on hv";

#[test]
fn the_note_says_processes_are_intact_when_the_agent_was_reattached() {
    let (w, engine, id) = answering(vec![Ok(AgentFate::Running)]);
    woken_after_suspend(&w, id, CRITICAL);
    assert_eq!(
        notes(&engine),
        [
            "This Box was suspended for 10s because host memory ran low. Its processes and \
          files are intact, but network connections open at that moment were dropped (API calls, \
          downloads, git pushes). Carry on with what you were doing. Re-run anything that failed."
        ]
    );
}

#[test]
fn a_reattached_agent_gets_its_keeper_back_and_keeps_its_terminal() {
    let (mut w, engine, id) = answering(vec![Ok(AgentFate::Running)]);
    let host = FakeProxyHost::new();
    w.cx = w.cx.with_proxy_host(Arc::new(host.clone()));
    woken_after_suspend(&w, id, CRITICAL);
    let restores = host
        .calls()
        .into_iter()
        .filter(|c| *c == ProxyHostCall::Restore(id))
        .count();
    assert_eq!(restores, 1);
    assert_eq!(engine.launches().len(), 1, "no launch over the live agent");
    assert_eq!(engine.reattached().len(), 1);
}

#[test]
fn the_note_says_processes_were_lost_when_the_agent_was_relaunched() {
    let (w, engine, id) = answering(vec![Ok(AgentFate::Ended { exit: None })]);
    woken_after_suspend(&w, id, CRITICAL);
    assert_eq!(engine.launches().len(), 2);
    assert_eq!(
        notes(&engine),
        [
            "This Box was suspended for 10s because host memory ran low. Its processes were \
          lost and its agent was started again, so anything that was running has stopped; its \
          files are kept. Carry on with what you were doing. Re-run anything that failed."
        ]
    );
}

#[test]
fn the_note_names_a_stopped_machine_rather_than_memory() {
    let (w, engine, id) = answering(vec![Ok(AgentFate::Ended { exit: None })]);
    woken_after_suspend(&w, id, moor_core::decide::reconcile::STOPPED_REASON);
    let said = notes(&engine);
    let first = said.first().map(String::as_str).unwrap_or_default();
    assert!(
        first.starts_with(
            "This Box was down for 10s: its machine stopped, as when its host restarts. \
             Its processes were lost"
        ),
        "{first}"
    );
}

#[test]
fn no_note_goes_out_until_the_box_answers_and_the_mark_stays() {
    let (w, engine, id) = answering(silent());
    woken_after_suspend(&w, id, CRITICAL);
    assert_eq!(notes(&engine), Vec::<String>::new());
    let store = w.cx.store().unwrap();
    assert!(store.resume_first(id).unwrap().is_some());
}

#[test]
fn a_host_restart_that_lost_the_agent_relaunches_it_and_says_so() {
    let (w, engine, id) = answering(vec![Ok(AgentFate::Ended { exit: None })]);
    let store = w.cx.store().unwrap();
    let handle = store
        .box_row(id)
        .unwrap()
        .unwrap()
        .boxed
        .unwrap()
        .handle
        .unwrap();
    w.driver.stop_behind(&handle);
    crate::ops::common::reconcile::run(&w.cx);
    let row = store.box_row(id).unwrap().unwrap();
    assert!(store.resume_first(id).unwrap().is_some());
    gone(&w);
    wake_and_revive(&w.cx, store, &row).unwrap();
    assert_eq!(engine.launches().len(), 2);
    assert_eq!(
        engine.launches().get(1).and_then(|l| l.resume.as_deref()),
        Some("agent-1")
    );
    assert!(store.resume_first(id).unwrap().is_none());
    let said = notes(&engine);
    assert!(
        said.first()
            .is_some_and(|n| n.contains("processes were lost")),
        "{said:?}"
    );
}

/// A Box suspended mid-turn whose machine then vanished, promoted onto a new one.
fn replaced_machine(w: &World, id: BoxId) {
    let store = w.cx.store().unwrap();
    let row = store.box_row(id).unwrap().unwrap();
    let handle = row.boxed.unwrap().handle.unwrap();
    w.driver.stop_behind(&handle);
    crate::ops::common::reconcile::run(&w.cx);
    assert!(store.resume_first(id).unwrap().is_some());
    w.driver.forget(&handle);
    crate::ops::common::reconcile::run(&w.cx);
    let node = store.box_spec(id).unwrap().node.unwrap();
    crate::ops::common::promote::promote_waiting(&w.cx, store, &node, "test");
}

const LOST_NOTE: &str = "This Box's machine was lost and a new one replaced it, so its disk is \
                         new: files and commits not pushed, and every process, are gone. Check \
                         what is there before you carry on.";

#[test]
fn a_stopped_box_whose_machine_is_gone_gets_a_new_one_and_leaves_the_queue() {
    let (w, _engine, id) = answering(vec![]);
    replaced_machine(&w, id);
    let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert_eq!(
        row.lifecycle,
        moor_core::domain::box_state::BoxLifecycle::Ready,
        "{:?}",
        w.cx.take_warnings()
    );
    assert_eq!(row.intent, None);
}

#[test]
fn a_replaced_machine_records_the_lost_disk_and_clears_the_resume_mark() {
    let (w, _engine, id) = answering(vec![]);
    replaced_machine(&w, id);
    let store = w.cx.store().unwrap();
    assert!(store.newest_event_of(id, "disk_lost").unwrap().is_some());
    assert_eq!(store.resume_first(id).unwrap(), None);
}

#[test]
fn the_first_agent_started_on_a_replaced_machine_is_told_and_only_that_one() {
    let (w, engine, id) = answering(vec![]);
    gone(&w);
    replaced_machine(&w, id);
    assert_eq!(engine.launches().len(), 2);
    assert_eq!(notes(&engine), [LOST_NOTE]);
    w.surface.push_pane_obs("p-2", Err(SurfaceError::NotFound));
    revive(&w, id);
    assert_eq!(engine.launches().len(), 3);
    assert_eq!(notes(&engine), [LOST_NOTE], "told once");
}

#[test]
fn a_start_that_replaces_a_lost_machine_records_the_disk_and_tells_the_agent() {
    let (w, engine, id) = answering(vec![]);
    let store = w.cx.store().unwrap();
    let row = store.box_row(id).unwrap().unwrap();
    w.driver.forget(&row.boxed.unwrap().handle.unwrap());
    crate::ops::common::reconcile::run(&w.cx);
    gone(&w);
    run(&w.cx, &super::req("demo")).unwrap();
    assert!(store.newest_event_of(id, "disk_lost").unwrap().is_some());
    assert_eq!(notes(&engine), [LOST_NOTE]);
}
