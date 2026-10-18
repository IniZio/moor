//! Tests for the capacity decider.

use std::num::NonZeroU32;

use super::{admit, promote_order, Admission, CapacityFacts, HeldBox};
use crate::domain::boxes::BoxState;
use crate::name::BoxId;

fn held(holder: &str, state: BoxState) -> HeldBox {
    HeldBox {
        holder: holder.to_owned(),
        state,
    }
}

fn facts(limit: Option<u32>, boxes: Vec<HeldBox>) -> CapacityFacts {
    CapacityFacts {
        limit: limit.and_then(NonZeroU32::new),
        boxes,
    }
}

fn ids(raw: &[i64]) -> Vec<BoxId> {
    raw.iter().map(|&v| BoxId::new(v)).collect()
}

#[test]
fn only_starting_ready_asleep_count() {
    for state in [BoxState::Starting, BoxState::Ready, BoxState::Asleep] {
        let f = facts(Some(1), vec![held("a", state)]);
        assert!(
            matches!(admit(&f), Admission::AtCapacity { .. }),
            "{state:?}"
        );
    }
    for state in [
        BoxState::Failed,
        BoxState::Gone,
        BoxState::ReleaseFailed,
        BoxState::Deleted,
    ] {
        let f = facts(Some(1), vec![held("a", state)]);
        assert_eq!(admit(&f), Admission::Admitted, "{state:?}");
    }
}

#[test]
fn at_limit_names_limit_and_holder() {
    let f = facts(
        Some(2),
        vec![
            held("gone", BoxState::Gone),
            held("one", BoxState::Ready),
            held("two", BoxState::Asleep),
        ],
    );
    assert_eq!(
        admit(&f),
        Admission::AtCapacity {
            holder: "one".to_owned(),
            limit: NonZeroU32::new(2).unwrap()
        }
    );
}

#[test]
fn under_limit_is_admitted() {
    let f = facts(Some(2), vec![held("one", BoxState::Ready)]);
    assert_eq!(admit(&f), Admission::Admitted);
}

#[test]
fn no_limit_always_admits() {
    let boxes = (0..50)
        .map(|i| held(&format!("t{i}"), BoxState::Ready))
        .collect();
    assert_eq!(admit(&facts(None, boxes)), Admission::Admitted);
}

#[test]
fn promote_returns_at_most_free_ascending() {
    assert_eq!(promote_order(&ids(&[9, 3, 7, 5]), 2), ids(&[3, 5]));
}

#[test]
fn promote_with_fewer_waiting_than_free() {
    assert_eq!(promote_order(&ids(&[4, 2]), 5), ids(&[2, 4]));
}

#[test]
fn promote_with_no_free_slots_is_empty() {
    assert!(promote_order(&ids(&[1, 2]), 0).is_empty());
}
