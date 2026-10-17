//! Tests for the reconcile decider.

use super::{reconcile, BoxFact};
use crate::domain::boxes::{BoxInput, BoxState};
use crate::domain::decision::TaskTarget;
use crate::name::{BoxHandle, TaskId};
use crate::seam::driver::{BoxListing, Platform};

fn fact(n: i64, state: BoxState, h: Option<&str>) -> BoxFact {
    BoxFact {
        task: TaskId::new(n),
        state,
        handle: h.map(BoxHandle::new),
    }
}

fn listed(h: &str, asleep: bool) -> BoxListing {
    BoxListing {
        handle: BoxHandle::new(h),
        provider: None,
        asleep,
        platform: Platform::Linux,
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
    assert_eq!(tasks, [TaskTarget::Existing(TaskId::new(2))]);
}
