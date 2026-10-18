//! Tests for the reconcile decider.

use super::{reconcile, BoxFact};
use crate::domain::boxes::{BoxInput, BoxState};
use crate::domain::decision::TaskTarget;
use crate::name::{BoxHandle, BoxId};
use crate::seam::driver::BoxListing;

fn fact(n: i64, state: BoxState, h: Option<&str>) -> BoxFact {
    BoxFact {
        task: BoxId::new(n),
        state,
        handle: h.map(BoxHandle::new),
    }
}

fn listed(h: &str, asleep: bool) -> BoxListing {
    BoxListing {
        handle: BoxHandle::new(h),
        provider: None,
        id: None,
        asleep,
    }
}

fn listed_as(h: &str, id: i64) -> BoxListing {
    BoxListing {
        id: Some(BoxId::new(id)),
        ..listed(h, false)
    }
}

#[test]
fn missing_ready_and_asleep_become_gone() {
    let rows = [
        fact(1, BoxState::Ready, Some("a")),
        fact(2, BoxState::Asleep, Some("b")),
    ];
    let out = reconcile(&rows, &[]);
    assert_eq!(out.len(), 2);
    for (d, r) in out.iter().zip(&rows) {
        assert_eq!(d.box_input, Some(BoxInput::DriverMissing));
        assert_eq!(d.task_input, None);
        assert_eq!(
            r.state.step(&BoxInput::DriverMissing).ok(),
            Some(BoxState::Gone)
        );
    }
}

#[test]
fn listed_boxes_are_untouched_even_on_sleep_mismatch() {
    let rows = [
        fact(1, BoxState::Ready, Some("a")),
        fact(2, BoxState::Asleep, Some("b")),
    ];
    let out = reconcile(&rows, &[listed("a", true), listed("b", false)]);
    assert!(out.is_empty());
}

#[test]
fn starting_without_handle_is_mid_acquire() {
    let rows = [
        fact(1, BoxState::Starting, None),
        fact(2, BoxState::Starting, Some("x")),
    ];
    assert!(reconcile(&rows, &[]).is_empty());
}

#[test]
fn terminal_and_failed_rows_are_skipped() {
    let rows = [
        fact(1, BoxState::Failed, Some("a")),
        fact(2, BoxState::Gone, Some("b")),
        fact(3, BoxState::Deleted, Some("c")),
        fact(4, BoxState::ReleaseFailed, Some("d")),
    ];
    assert!(reconcile(&rows, &[]).is_empty());
}

#[test]
fn ready_row_without_handle_is_left_alone() {
    assert!(reconcile(&[fact(1, BoxState::Ready, None)], &[]).is_empty());
}

#[test]
fn only_the_missing_row_is_decided() {
    let rows = [
        fact(1, BoxState::Ready, Some("a")),
        fact(2, BoxState::Ready, Some("b")),
    ];
    let out = reconcile(&rows, &[listed("a", false)]);
    let tasks: Vec<_> = out.iter().map(|d| d.task.clone()).collect();
    assert_eq!(tasks, [TaskTarget::Existing(BoxId::new(2))]);
}

#[test]
fn live_box_named_with_a_slash_stays_live() {
    let rows = [fact(7, BoxState::Ready, Some("prov/moor-7-a1b2c3"))];
    let out = reconcile(&rows, &[listed_as("prov/MOOR-7-A1B2C3", 7)]);
    assert!(out.is_empty());
}

#[test]
fn differing_handle_with_another_box_id_is_gone() {
    let rows = [fact(7, BoxState::Ready, Some("prov/moor-7-a1b2c3"))];
    let out = reconcile(&rows, &[listed_as("prov/moor-8-a1b2c3", 8)]);
    let tasks: Vec<_> = out.iter().map(|d| d.task.clone()).collect();
    assert_eq!(tasks, [TaskTarget::Existing(BoxId::new(7))]);
}
