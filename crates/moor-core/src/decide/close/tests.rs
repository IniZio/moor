//! Tests for the close decider.

use super::{close_gate, release_outcome, CloseFacts, Refusal, ReleaseResult, ReleaseVerdict};
use crate::domain::boxes::BoxState;
use crate::domain::task::{TaskInput, TaskState};
use crate::name::TaskId;

use BoxState as B;
use ReleaseResult as R;

/// Clean facts for a working task with a Ready Box.
fn facts() -> CloseFacts {
    CloseFacts {
        task: TaskId::new(7),
        state: TaskState::Working,
        box_state: Some(B::Ready),
        has_handle: true,
        unpushed: 0,
        dirty: false,
    }
}

#[test]
fn clean_ready_box_is_released_and_closed() {
    let plan = close_gate(&facts(), false).ok();
    let plan = plan.expect("plan");
    assert!(plan.release_box && plan.close_workspace);
    assert_eq!(plan.decision_close.task_input, Some(TaskInput::Close));
}

#[test]
fn unsaved_work_is_refused_without_force() {
    for (unpushed, dirty) in [(1, false), (0, true), (3, true)] {
        let f = CloseFacts {
            unpushed,
            dirty,
            ..facts()
        };
        assert_eq!(
            close_gate(&f, false).err(),
            Some(Refusal::UnpushedWork { unpushed, dirty })
        );
        assert!(close_gate(&f, true).is_ok());
    }
}

#[test]
fn refusal_text_suggests_force_and_names_no_driver() {
    let text = Refusal::UnpushedWork {
        unpushed: 2,
        dirty: true,
    }
    .to_string();
    assert!(text.contains("--force") && text.contains('2'), "{text}");
}

#[test]
fn closed_and_rejected_tasks_are_refused() {
    for state in [TaskState::Closed, TaskState::Rejected] {
        let f = CloseFacts { state, ..facts() };
        assert_eq!(close_gate(&f, true).err(), Some(Refusal::AlreadyClosed));
    }
}

#[test]
fn every_live_state_closes() {
    for state in TaskState::ALL.into_iter().filter(|s| s.is_live()) {
        let f = CloseFacts { state, ..facts() };
        assert!(close_gate(&f, false).is_ok(), "{state:?}");
    }
}

#[test]
fn release_is_planned_only_for_a_releasable_box_with_a_handle() {
    let cases = [
        (None, true, false),
        (Some(B::Starting), false, false),
        (Some(B::Starting), true, false),
        (Some(B::Ready), true, true),
        (Some(B::Asleep), true, true),
        (Some(B::Failed), true, true),
        (Some(B::Failed), false, false),
        (Some(B::Gone), true, false),
        (Some(B::ReleaseFailed), true, true),
        (Some(B::Deleted), true, false),
    ];
    for (box_state, has_handle, want) in cases {
        let f = CloseFacts {
            box_state,
            has_handle,
            ..facts()
        };
        let plan = close_gate(&f, false).expect("plan");
        assert_eq!(plan.release_box, want, "{box_state:?} handle={has_handle}");
    }
}

/// Applies a verdict's decision to the machines, returning the final states.
fn apply(v: &ReleaseVerdict, task: TaskState, b: BoxState) -> (TaskState, BoxState) {
    let d = v.decision.as_ref().expect("decision");
    let t = d.task_input.as_ref().map_or(Ok(task), |i| task.step(i));
    let bx = d.box_input.as_ref().map_or(Ok(b), |i| b.step(i));
    (t.expect("task step"), bx.expect("box step"))
}

#[test]
fn deleted_and_already_gone_mark_the_box_deleted() {
    for r in [R::Deleted, R::AlreadyGone] {
        for b in [B::Ready, B::Asleep, B::Failed, B::ReleaseFailed] {
            let v = release_outcome(TaskId::new(7), b, r);
            assert!(v.closes && v.warning.is_none());
            assert_eq!(
                apply(&v, TaskState::Working, b),
                (TaskState::Closed, B::Deleted)
            );
        }
    }
}

#[test]
fn failed_release_of_a_live_box_keeps_the_task_open() {
    for b in [B::Ready, B::Asleep] {
        let v = release_outcome(TaskId::new(7), b, R::Failed);
        assert!(!v.closes && v.decision.is_none() && v.warning.is_none());
    }
}

#[test]
fn failed_release_of_a_failed_box_closes_with_release_failed_and_warns() {
    let v = release_outcome(TaskId::new(7), B::Failed, R::Failed);
    assert!(v.closes);
    assert_eq!(
        apply(&v, TaskState::SetupFailed, B::Failed),
        (TaskState::Closed, B::ReleaseFailed)
    );
    let w = v.warning.expect("warning");
    assert_eq!(w.code, "box_release_failed");
    assert!(!w.message.contains("RAWDRIVER"));
    assert!(w.message.contains("not deleted") && w.message.contains("handle kept"));
}

#[test]
fn failed_release_of_a_release_failed_box_stays_release_failed() {
    let v = release_outcome(TaskId::new(7), B::ReleaseFailed, R::Failed);
    assert!(v.closes && v.warning.is_some());
    assert_eq!(
        apply(&v, TaskState::SetupFailed, B::ReleaseFailed),
        (TaskState::Closed, B::ReleaseFailed)
    );
}
