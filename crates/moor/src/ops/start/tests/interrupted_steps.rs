//! Tests for a restart that cut a start after its setup lines passed, on the steps after them.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::{Decision, Field, Fields, Intent};
use moor_core::BoxId;

use super::interrupted::{box_row, restarted_mid_setup, setup_attaches};
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
    assert_eq!(row.intent, None);
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
        .apply(Decision::new(BoxId::new(1)).set(Fields {
            box_handle: Field::Clear,
            ..Fields::default()
        }))
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
    assert_eq!(row.intent, None);
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
    assert!(matches!(row.intent, Some(Intent::Setup { .. })));
    assert_eq!(setup_why_of(&w), None);
}

#[test]
fn later_steps_cut_on_a_machine_that_is_not_ready_fail_the_setup_with_why() {
    let (w, _) = restarted_mid_setup("setup: [a]\n");
    let store = w.cx.store().unwrap();
    store
        .apply(Decision::new(BoxId::new(1)).set(Fields {
            steps_pending_at: Field::Set(1),
            box_workdir: Field::Clear,
            ..Fields::default()
        }))
        .unwrap();
    assert!(fail_interrupted(&w.cx, store).unwrap().is_empty());
    let row = box_row(&w);
    assert_eq!(row.lifecycle, BoxLifecycle::SetupFailed);
    assert_eq!(row.intent, None);
    let why = setup_why_of(&w).expect("a why");
    assert!(why.contains("not ready"), "{why}");
    let events = store.events(row.id, 200).unwrap();
    assert!(!events.iter().any(|e| e.kind == "working"));
}
