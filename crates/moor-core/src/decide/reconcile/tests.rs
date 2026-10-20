//! Tests for the reconcile decider.

use super::{reconcile, BoxFact};
use crate::domain::boxes::{BoxInput, BoxState};
use crate::domain::decision::BoxTarget;
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
        asleep,
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
        assert_eq!(d.lifecycle_input, None);
        assert_eq!(d.session_input, None);
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
    let tasks: Vec<_> = out.iter().map(|d| d.target.clone()).collect();
    assert_eq!(tasks, [BoxTarget::Existing(BoxId::new(2))]);
}

#[test]
fn foreign_moor_slug_does_not_keep_a_row_live() {
    let rows = [fact(7, BoxState::Ready, Some("prov/moor-7-a1b2c3"))];
    let l = listed("prov/moor-7-ffffff", false);
    let out = reconcile(&rows, &[l]);
    let tasks: Vec<_> = out.iter().map(|d| d.target.clone()).collect();
    assert_eq!(tasks, [BoxTarget::Existing(BoxId::new(7))]);
}

#[test]
fn digit_bare_name_slug_does_not_keep_a_row_live() {
    let rows = [fact(42, BoxState::Ready, Some("prov/moor-42-0c0c0c"))];
    let l = listed("prov/moor-42-a1b2c3", false);
    let out = reconcile(&rows, &[l]);
    let tasks: Vec<_> = out.iter().map(|d| d.target.clone()).collect();
    assert_eq!(tasks, [BoxTarget::Existing(BoxId::new(42))]);
}
