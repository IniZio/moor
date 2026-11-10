//! Tests for a restart that cut a start after its setup lines passed, on the steps after them.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::{Decision, Effect, Field, Fields, HandsFields};
use moor_core::domain::hands::HandsInc;
use moor_core::BoxId;

use super::agent_install::{install_cut_off, installs};
use super::interrupted::{box_row, restarted_mid_setup, setup_attaches};
use super::setup::unwrapped;
use super::World;
use crate::ops::common::provision::interrupted::fail_interrupted;
use crate::ops::common::provision::resume_setup;

#[test]
fn a_restart_once_the_setup_lines_passed_finishes_the_later_steps_and_launches_the_agent() {
    let (w, attached) = restarted_mid_setup("setup: [a]\n");
    let store = w.cx.store().unwrap();
    store
        .apply(Decision::new(BoxId::new(1)).set(Fields {
            steps_pending_at: Field::Set(1),
            ..Fields::default()
        }))
        .unwrap();
    let cut = store.events(BoxId::new(1), 200).unwrap().last().unwrap().id;
    assert_eq!(fail_interrupted(&w.cx, store).unwrap(), [BoxId::new(1)]);
    assert_eq!(box_row(&w).lifecycle, BoxLifecycle::SettingUp);
    resume_setup(&w.cx, store, BoxId::new(1)).unwrap();
    let row = box_row(&w);
    assert_eq!(row.lifecycle, BoxLifecycle::Ready);
    assert_eq!(row.effect, None);
    assert_eq!(row.steps_pending_at, None);
    let after: Vec<(String, String)> = store
        .events(row.id, 200)
        .unwrap()
        .into_iter()
        .filter(|e| e.id > cut)
        .map(|e| {
            let step = e.payload.get("step").and_then(|s| s.as_str()).unwrap_or("");
            (e.kind, step.to_owned())
        })
        .collect();
    let at = |kind: &str, step: &str| after.iter().position(|(k, s)| k == kind && s == step);
    let working = at("working", "").expect("working");
    let launched = at("step_started", "launch").expect("launch began");
    let session = at("session_started", "").expect("a session started");
    assert!(working < launched && launched < session, "{after:?}");
    assert!(after.iter().all(|(k, _)| k != "idled"), "{after:?}");
    assert_eq!(
        setup_attaches(&w),
        [attached],
        "the passed lines are not followed again"
    );
}

/// A world whose restart cut the steps after the passed setup lines, with the cut start claimed
/// and its machine then lost.
fn claimed_then_machine_lost() -> World {
    let (w, _) = restarted_mid_setup("setup: [a]\n");
    let store = w.cx.store().unwrap();
    store
        .apply(Decision::new(BoxId::new(1)).set(Fields {
            steps_pending_at: Field::Set(1),
            ..Fields::default()
        }))
        .unwrap();
    assert_eq!(fail_interrupted(&w.cx, store).unwrap(), [BoxId::new(1)]);
    store
        .apply(Decision::new(BoxId::new(1)).hands(
            HandsInc::FIRST,
            HandsFields {
                handle: Field::Clear,
                ..HandsFields::default()
            },
        ))
        .unwrap();
    w
}

fn setup_why_of(w: &World) -> Option<String> {
    let events = w.cx.store().unwrap().events(BoxId::new(1), 200).unwrap();
    let failed = events.iter().rfind(|e| e.kind == "setup_failed")?;
    failed.payload.get("why")?.as_str().map(str::to_owned)
}

#[test]
fn later_steps_that_cannot_finish_after_a_restart_fail_the_setup_with_why() {
    let w = claimed_then_machine_lost();
    let store = w.cx.store().unwrap();
    assert!(resume_setup(&w.cx, store, BoxId::new(1)).is_err());
    let row = box_row(&w);
    assert_eq!(row.lifecycle, BoxLifecycle::SetupFailed);
    assert_eq!(row.effect, None);
    let why = setup_why_of(&w).expect("a why");
    assert!(!why.is_empty());
    let events = store.events(row.id, 200).unwrap();
    assert!(!events.iter().any(|e| e.kind == "session_started"));
}

#[test]
fn a_drain_leaves_later_steps_that_cannot_finish_to_the_next_process() {
    let w = claimed_then_machine_lost();
    let store = w.cx.store().unwrap();
    w.cx.env().draining().cancel();
    assert!(resume_setup(&w.cx, store, BoxId::new(1)).is_err());
    let row = box_row(&w);
    assert_eq!(row.lifecycle, BoxLifecycle::SettingUp);
    assert!(matches!(row.effect, Some(Effect::Setup)));
    assert_eq!(setup_why_of(&w), None);
}

#[test]
fn later_steps_cut_on_a_machine_that_is_not_ready_fail_the_setup_with_why() {
    let (w, _) = restarted_mid_setup("setup: [a]\n");
    let store = w.cx.store().unwrap();
    store
        .apply(
            Decision::new(BoxId::new(1))
                .set(Fields {
                    steps_pending_at: Field::Set(1),
                    ..Fields::default()
                })
                .hands(
                    HandsInc::FIRST,
                    HandsFields {
                        workdir: Field::Clear,
                        ..HandsFields::default()
                    },
                ),
        )
        .unwrap();
    assert!(fail_interrupted(&w.cx, store).unwrap().is_empty());
    let row = box_row(&w);
    assert_eq!(row.lifecycle, BoxLifecycle::SetupFailed);
    assert_eq!(row.effect, None);
    let why = setup_why_of(&w).expect("a why");
    assert!(why.contains("not ready"), "{why}");
    let events = store.events(row.id, 200).unwrap();
    assert!(!events.iter().any(|e| e.kind == "working"));
}

/// Execs that asked whether the agent is there.
fn presence_checks(w: &World) -> usize {
    w.driver
        .execs()
        .into_iter()
        .filter(|e| unwrapped(e.1.argv.clone()) == ["sh", "-c", "true"])
        .count()
}

#[test]
fn a_restart_after_the_agent_landed_skips_the_install_and_launches() {
    let (w, engine) = install_cut_off();
    resume_setup(&w.cx, w.cx.store().unwrap(), BoxId::new(1)).unwrap();
    assert_eq!(presence_checks(&w), 1);
    assert!(installs(&w).is_empty(), "the agent is already there");
    assert_eq!(engine.launches().len(), 1);
    let row =
        w.cx.store()
            .unwrap()
            .box_row(BoxId::new(1))
            .unwrap()
            .unwrap();
    assert_eq!(row.lifecycle, BoxLifecycle::Ready);
}

#[test]
fn a_restart_during_the_install_is_not_a_failed_setup() {
    let (w, engine) = install_cut_off();
    assert!(engine.launches().is_empty());
    resume_setup(&w.cx, w.cx.store().unwrap(), BoxId::new(1)).unwrap();
    assert_eq!(engine.launches().len(), 1);
    let row =
        w.cx.store()
            .unwrap()
            .box_row(BoxId::new(1))
            .unwrap()
            .unwrap();
    assert_eq!(row.lifecycle, BoxLifecycle::Ready);
    assert_eq!(row.effect, None);
    assert_eq!(row.setup_failed_at, None);
    let events = w.cx.store().unwrap().events(BoxId::new(1), 50).unwrap();
    assert!(
        events.iter().all(|e| e.kind != "setup_failed"),
        "{events:?}"
    );
}
