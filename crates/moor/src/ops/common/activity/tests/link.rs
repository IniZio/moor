//! An agent pane that ends while the agent still runs on the Box keeps the session.

use std::sync::Arc;

use moor_core::config::AgentHolder;
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::engine::{AgentFate, EngineError};
use moor_core::BoxId;

use super::exit::exited_probes;
use super::turn::steer;
use super::{cx, is_working, row, run_once, working_box, SESSION};
use crate::cx::Cx;
use crate::ops::common::activity::settle;
use crate::ops::common::agent_stop::stop_agent;
use crate::testing::engine::FakeEngine;

/// A working Box in workspace `ws` whose live session runs in pane `p1`.
fn attached_box(cx: &Cx) -> BoxId {
    attached_box_under(cx, AgentHolder::Hc)
}

/// As [`attached_box`], with the agent's terminal held by `kind`.
fn attached_box_under(cx: &Cx, kind: AgentHolder) -> BoxId {
    let id = working_box(cx, false);
    let store = cx.store().unwrap();
    let set = Fields {
        workspace_id: Field::Set("ws".to_owned()),
        ..Fields::default()
    };
    store.apply(Decision::new(id).set(set)).unwrap();
    store.begin_session(id, SESSION, Some("p1"), kind).unwrap();
    id
}

/// Settles one reading of a Box whose pane ended with exit 1, given what the Box answers.
fn settle_exited(fates: Vec<Result<AgentFate, EngineError>>) -> (Cx, Arc<FakeEngine>, BoxId) {
    settle_exited_after(fates, |_, _| {})
}

/// As [`settle_exited`], with `before` run on the Box ahead of the reading.
fn settle_exited_after(
    fates: Vec<Result<AgentFate, EngineError>>,
    before: impl FnOnce(&Cx, BoxId),
) -> (Cx, Arc<FakeEngine>, BoxId) {
    let dir = tempfile::tempdir().unwrap().keep();
    let engine = Arc::new(
        FakeEngine::new()
            .probes(SESSION, exited_probes())
            .transcript_text("abc")
            .fates(fates),
    );
    let (cx, _) = cx(&dir, &engine);
    let id = attached_box(&cx);
    before(&cx, id);
    let reading = run_once(&cx, id);
    settle(&cx, cx.store().unwrap(), &row(&cx, id), &reading).unwrap();
    (cx, engine, id)
}

/// The kinds of `id`'s events from the given ones on, oldest first.
fn kinds(cx: &Cx, id: BoxId, of: &[&str]) -> Vec<(String, serde_json::Value)> {
    let mut events = cx.store().unwrap().events(id, 100).unwrap();
    events.sort_by_key(|e| e.id);
    events
        .into_iter()
        .filter(|e| of.contains(&e.kind.as_str()))
        .map(|e| (e.kind, e.payload))
        .collect()
}

const WATCHED: &[&str] = &[
    "agent_link_lost",
    "agent_link_restored",
    "agent_unreachable",
    "session_ended",
    "transcript_saved",
    "transcript_skipped",
];

fn unreachable() -> Result<AgentFate, EngineError> {
    Err(EngineError::Failed {
        step: "fate",
        message: "ssh: connection refused".to_owned(),
    })
}

#[test]
fn a_running_agent_gets_a_new_pane_and_keeps_its_session() {
    let (cx, engine, id) = settle_exited(vec![Ok(AgentFate::Running)]);
    let seen = kinds(&cx, id, WATCHED);
    let names: Vec<&str> = seen.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(names, ["agent_link_lost", "agent_link_restored"]);
    let (lost, restored) = (&seen.first().unwrap().1, &seen.last().unwrap().1);
    assert_eq!(lost.get("pane").unwrap(), "p1");
    assert_eq!(lost.get("exit").unwrap(), 1);
    assert_eq!(restored.get("pane").unwrap(), "reattached-1");
    let live = cx.store().unwrap().live_session(id).unwrap().unwrap();
    assert_eq!(live.pane.as_deref(), Some("reattached-1"));
    assert_eq!(engine.reattached().len(), 1);
    assert!(engine.transcripts().is_empty());
}

/// The calls reaching the agent launched under `launched` (`None`: before holders were
/// recorded) from a controller that now launches under `configured`.
fn asked_after_the_holder_changed(
    launched: Option<AgentHolder>,
    configured: AgentHolder,
) -> Vec<(&'static str, AgentHolder)> {
    let dir = tempfile::tempdir().unwrap().keep();
    let engine = Arc::new(
        FakeEngine::new()
            .probes(SESSION, exited_probes())
            .fates(vec![Ok(AgentFate::Running)]),
    );
    let (cx, _) = cx(&dir, &engine);
    let id = attached_box_under(&cx, launched.unwrap_or(configured));
    if launched.is_none() {
        cx.store().unwrap().forget_holders(id).unwrap();
    }
    engine.set_holder(configured);
    let reading = run_once(&cx, id);
    let store = cx.store().unwrap();
    settle(&cx, store, &row(&cx, id), &reading).unwrap();
    stop_agent(&cx, store, &row(&cx, id), None).unwrap();
    engine.asked()
}

#[test]
fn a_box_launched_under_tmux_stays_with_tmux_after_the_controller_switches_to_hc() {
    let tmux = AgentHolder::Tmux;
    assert_eq!(
        asked_after_the_holder_changed(Some(tmux), AgentHolder::Hc),
        [("fate", tmux), ("reattach", tmux), ("stop", tmux)]
    );
}

#[test]
fn a_box_launched_under_hc_stays_with_hc_after_the_controller_switches_to_tmux() {
    let hc = AgentHolder::Hc;
    assert_eq!(
        asked_after_the_holder_changed(Some(hc), AgentHolder::Tmux),
        [("fate", hc), ("reattach", hc), ("stop", hc)]
    );
}

#[test]
fn a_box_launched_before_holders_were_recorded_is_held_by_tmux() {
    let tmux = AgentHolder::Tmux;
    assert_eq!(
        asked_after_the_holder_changed(None, AgentHolder::Hc),
        [("fate", tmux), ("reattach", tmux), ("stop", tmux)]
    );
}

#[test]
fn a_reattached_agent_stays_working() {
    let (cx, _, id) = settle_exited(vec![Ok(AgentFate::Running)]);
    assert!(kinds(&cx, id, &["idled"]).is_empty());
    assert!(is_working(&row(&cx, id)));
}

#[test]
fn a_reattached_agent_keeps_its_steered_turn_open() {
    let (cx, _, id) = settle_exited_after(vec![Ok(AgentFate::Running)], |cx, id| {
        steer(cx, id);
    });
    assert!(kinds(&cx, id, &["turn_done"]).is_empty());
    let store = cx.store().unwrap();
    assert!(store.unanswered_steer(id).unwrap().is_some());
}

#[test]
fn an_ended_agent_ends_the_session_with_the_exit_the_box_kept() {
    let (cx, engine, id) = settle_exited(vec![Ok(AgentFate::Ended { exit: Some(42) })]);
    let seen = kinds(&cx, id, WATCHED);
    let ended = seen.iter().find(|(k, _)| k == "session_ended").unwrap();
    assert_eq!(ended.1.get("exit").unwrap(), 42);
    assert!(!seen.iter().any(|(k, _)| k.starts_with("agent_link")));
    assert!(engine.reattached().is_empty());
    assert!(cx.store().unwrap().live_session(id).unwrap().is_none());
}

#[test]
fn a_box_that_stays_unreachable_ends_the_session_and_says_so() {
    let (cx, engine, id) = settle_exited((0..8).map(|_| unreachable()).collect());
    cx.clock().sleep(std::time::Duration::from_secs(600));
    let reading = run_once(&cx, id);
    settle(&cx, cx.store().unwrap(), &row(&cx, id), &reading).unwrap();
    let seen = kinds(&cx, id, WATCHED);
    let lost = seen.iter().find(|(k, _)| k == "agent_unreachable").unwrap();
    assert_eq!(lost.1.get("attempts").unwrap(), 4);
    assert!(lost
        .1
        .get("error")
        .and_then(serde_json::Value::as_str)
        .unwrap()
        .contains("connection refused"));
    let ended = seen.iter().find(|(k, _)| k == "session_ended").unwrap();
    assert_eq!(ended.1.get("exit").unwrap(), 1);
    assert!(engine.reattached().is_empty());
}

#[test]
fn a_box_reached_on_a_later_try_reattaches() {
    let (cx, engine, id) =
        settle_exited(vec![unreachable(), unreachable(), Ok(AgentFate::Running)]);
    let seen = kinds(&cx, id, WATCHED);
    assert!(!seen
        .iter()
        .any(|(k, _)| k == "agent_unreachable" || k == "session_ended"));
    assert_eq!(engine.reattached().len(), 1);
}

#[test]
fn the_agent_of_an_asleep_box_ends_without_asking_the_box() {
    let dir = tempfile::tempdir().unwrap().keep();
    let engine = Arc::new(
        FakeEngine::new()
            .probes(SESSION, exited_probes())
            .transcript_text("abc")
            .fates(vec![unreachable(); 8]),
    );
    let (cx, _) = cx(&dir, &engine);
    let id = attached_box(&cx);
    let store = cx.store().unwrap();
    store
        .apply(Decision::new(id).session(SessionInput::AgentQuiet))
        .unwrap();
    let sleep = BoxLifecycleInput::Sleep {
        auto: true,
        reason: None,
    };
    store
        .apply(
            Decision::new(id)
                .lifecycle(sleep)
                .box_input(BoxInput::Sleep),
        )
        .unwrap();
    let reading = run_once(&cx, id);
    settle(&cx, store, &row(&cx, id), &reading).unwrap();
    let seen = kinds(&cx, id, WATCHED);
    assert!(!seen.iter().any(|(k, _)| k == "agent_unreachable"));
    let ended = seen.iter().find(|(k, _)| k == "session_ended").unwrap();
    assert_eq!(ended.1.get("exit").unwrap(), 1);
    assert_eq!(engine.fates_left(), 8);
}

#[test]
fn a_transcript_that_failed_for_want_of_a_link_is_saved_once_the_agent_is_reattached() {
    let (cx, engine, id) = settle_exited_after(vec![Ok(AgentFate::Running)], |cx, id| {
        let failed = ExtraEvent::TranscriptFailed {
            trigger: "turn".to_owned(),
            error: "box: machine 2 is not linked".to_owned(),
            unlinked: true,
        };
        let store = cx.store().unwrap();
        let live = store.live_session(id).unwrap().unwrap();
        store.set_registered_session(live.id, "agent-1").unwrap();
        store.apply(Decision::new(id).event(failed)).unwrap();
    });
    assert_eq!(engine.transcripts().len(), 1);
    let saved = kinds(&cx, id, &["transcript_saved"]);
    assert_eq!(saved.len(), 1);
    assert_eq!(saved.first().unwrap().1.get("trigger").unwrap(), "turn");
}
