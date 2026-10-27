use moor_core::domain::decision::{Decision, Field, Fields};
use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::decision::ExtraEvent;
use moor_core::seam::proxy_host::ProxyStatus;
use moor_core::seam::surface::SurfaceError;
use moor_core::BoxId;

use super::dead_agent::{ca_stopped, recorded_failure, CA};
use super::relaunch::{gone, obs, set_agent_session, started};
use super::World;
use crate::ops::start::revive_after_wake;
use crate::testing::proxy_host::{FakeProxyHost, ProxyHostCall};

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
            .query_row(
                "SELECT exit FROM launches WHERE box = ? ORDER BY id LIMIT 1",
                [id.get()],
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
