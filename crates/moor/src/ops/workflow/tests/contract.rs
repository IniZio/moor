//! A run's first launch reads the run part of moor's contract, before the run names its Box.

use std::sync::Arc;

use super::{rig, FIRST_DUE};
use crate::testing::contract::{locked_version, OLD_TEXT};
use crate::testing::engine::FakeEngine;

#[test]
fn a_run_s_first_launch_reads_the_run_part_and_records_the_contract() {
    let engine = Arc::new(FakeEngine::new());
    let mut r = rig();
    r.cx = r.cx.with_engine(engine.clone());
    r.answer_repo();
    let id = r.put(180);
    r.advance_to(FIRST_DUE + 1_000);
    assert!(r.pass().is_empty());
    let launch = engine.launches().pop().expect("an agent was launched");
    assert!(launch.run.is_some(), "{launch:?}");
    let store = r.cx.store().unwrap();
    let events = store.events(r.latest_box(id), 200).unwrap();
    assert_eq!(
        events.iter().filter(|e| e.kind == "contract_given").count(),
        1
    );
}

#[test]
fn a_run_s_fresh_session_records_the_locked_contract_version_and_no_old_text_in_its_first_message()
{
    let engine = Arc::new(FakeEngine::new());
    let mut r = rig();
    r.cx = r.cx.with_engine(engine.clone());
    r.answer_repo();
    let id = r.put(180);
    r.advance_to(FIRST_DUE + 1_000);
    assert!(r.pass().is_empty());
    let first = engine.launches().pop().expect("an agent was launched");
    let said = first
        .prompt
        .text()
        .expect("a run's agent gets a first message");
    for old in OLD_TEXT {
        assert!(!said.contains(old), "{old:?} is still said:\n{said}");
    }
    let store = r.cx.store().unwrap();
    let given = store
        .first_event_of(r.latest_box(id), "contract_given")
        .unwrap()
        .expect("the run's Box recorded the contract it read");
    assert_eq!(
        given.payload.get("version"),
        Some(&serde_json::json!(locked_version()))
    );
}
