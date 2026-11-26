use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::event::ContractEvent;

use super::{new_box, open};

fn given(version: u32) -> ExtraEvent {
    ExtraEvent::Contract(ContractEvent::ContractGiven {
        version,
        sha256: "ab".into(),
        harness: "claude".into(),
    })
}

#[test]
fn the_first_event_of_a_kind_is_the_oldest_and_none_without_one() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let a = new_box(&store, "alpha");
    assert!(store.first_event_of(a, "contract_given").unwrap().is_none());
    for v in [1, 2] {
        store.apply(Decision::new(a).event(given(v))).unwrap();
    }
    let first = store.first_event_of(a, "contract_given").unwrap().unwrap();
    assert_eq!(first.payload.get("version"), Some(&serde_json::json!(1)));
}
