//! A run's first launch reads the run part of moor's contract, before the run names its Box.

use std::sync::Arc;

use super::{rig, FIRST_DUE};
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
