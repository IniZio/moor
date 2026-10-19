use moor_core::domain::event::TurnOutcome;

use super::{new_task, open};

#[test]
fn a_steer_gets_one_turn_done_however_often_it_is_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_task(&store, "alpha");
    assert!(store.record_turn_done(id, 7, TurnOutcome::Idle).unwrap());
    assert!(!store.record_turn_done(id, 7, TurnOutcome::Exited).unwrap());
    let done: Vec<_> = store
        .events(id, 100)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "turn_done")
        .collect();
    assert_eq!(done.len(), 1);
    assert_eq!(
        done.first().unwrap().payload,
        serde_json::json!({"steer": 7, "outcome": "idle"})
    );
    assert!(store.record_turn_done(id, 8, TurnOutcome::Idle).unwrap());
}
