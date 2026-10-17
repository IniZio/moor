//! Tests for the close decider.

use super::{
    close_gate, release_outcome, BoxCheck, CloseFacts, Refusal, ReleaseResult, ReleaseVerdict,
};
use crate::domain::boxes::{BoxInput, BoxState};
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
        name: "fix-bug".to_owned(),
        branch: Some("moor/fix-bug".to_owned()),
        unpushed: 0,
        dirty: false,
        box_check: BoxCheck::Clean,
    }
}

#[test]
fn box_with_unsaved_work_is_refused_with_recovery_commands() {
    let f = CloseFacts {
        box_check: BoxCheck::Unsaved { ahead: 2, dirty: 3 },
        ..facts()
    };
    let err = close_gate(&f, false).expect_err("refused");
    assert_eq!(
        err,
        Refusal::UnsavedInBox {
            task: "fix-bug".to_owned(),
            branch: "moor/fix-bug".to_owned(),
            ahead: 2,
            dirty: 3,
        }
    );
    let text = err.to_string();
    for want in [
        "2 unpushed commit(s)",
        "3 uncommitted file(s)",
        "not on the host",
        "`moor run fix-bug --no-sync -- git bundle create .moor/rescue.bundle HEAD --not --remotes`",
        "`moor cp fix-bug .moor/rescue.bundle .`",
        "`git fetch ./rescue.bundle HEAD:moor/fix-bug`",
        "`moor close fix-bug --force`",
    ] {
        assert!(text.contains(want), "{want}: {text}");
    }
}

#[test]
fn unsaved_box_with_one_side_nonzero_is_refused() {
    for (ahead, dirty) in [(1, 0), (0, 1)] {
        let f = CloseFacts {
            box_check: BoxCheck::Unsaved { ahead, dirty },
            ..facts()
        };
        assert!(matches!(
            close_gate(&f, false),
            Err(Refusal::UnsavedInBox { .. })
        ));
    }
}

#[test]
fn unsaved_box_with_zero_counts_clean_and_skipped_pass() {
    for box_check in [
        BoxCheck::Unsaved { ahead: 0, dirty: 0 },
        BoxCheck::Clean,
        BoxCheck::Skipped,
    ] {
        let f = CloseFacts {
            box_check,
            ..facts()
        };
        assert!(close_gate(&f, false).is_ok());
    }
}

#[test]
fn unknown_box_check_is_refused_with_its_reason() {
    let f = CloseFacts {
        box_check: BoxCheck::Unknown("ssh timed out".to_owned()),
        ..facts()
    };
    let err = close_gate(&f, false).expect_err("refused");
    assert_eq!(
        err,
        Refusal::BoxUnchecked {
            reason: "ssh timed out".to_owned()
        }
    );
    let text = err.to_string();
    assert!(
        text.contains("ssh timed out") && text.contains("--force"),
        "{text}"
    );
}

#[test]
fn force_ignores_the_box_check() {
    for box_check in [
        BoxCheck::Unsaved { ahead: 4, dirty: 1 },
        BoxCheck::Unknown("boom".to_owned()),
    ] {
        let f = CloseFacts {
            box_check,
            ..facts()
        };
        assert!(close_gate(&f, true).is_ok());
    }
}

#[test]
fn missing_branch_uses_a_placeholder() {
    let f = CloseFacts {
        branch: None,
        box_check: BoxCheck::Unsaved { ahead: 1, dirty: 0 },
        ..facts()
    };
    let err = close_gate(&f, false).expect_err("refused");
    assert!(
        matches!(&err, Refusal::UnsavedInBox { branch, .. } if branch == "<branch>"),
        "{err:?}"
    );
    assert!(err.to_string().contains("HEAD:<branch>"));
}

#[test]
fn host_unpushed_is_checked_before_the_box() {
    let f = CloseFacts {
        unpushed: 1,
        box_check: BoxCheck::Unsaved { ahead: 5, dirty: 5 },
        ..facts()
    };
    assert_eq!(
        close_gate(&f, false).err(),
        Some(Refusal::UnpushedWork {
            unpushed: 1,
            dirty: false
        })
    );
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

fn out(b: BoxState, r: ReleaseResult, force: bool) -> ReleaseVerdict {
    out_rows(b, r, force, true)
}

fn out_rows(b: BoxState, r: ReleaseResult, force: bool, rows_left: bool) -> ReleaseVerdict {
    release_outcome(
        TaskId::new(7),
        b,
        r,
        force,
        &LEFT.map(str::to_owned),
        rows_left,
    )
}

const LEFT: [&str; 2] = ["root /srv/t7", "process 4242"];

#[test]
fn deleted_and_already_gone_mark_the_box_deleted() {
    for r in [R::Deleted, R::AlreadyGone] {
        for b in [B::Ready, B::Asleep, B::Failed, B::ReleaseFailed] {
            let v = out(b, r, false);
            assert!(v.closes && v.warning.is_none());
            assert_eq!(
                apply(&v, TaskState::Working, b),
                (TaskState::Closed, B::Deleted)
            );
        }
    }
}

#[test]
fn a_plain_close_that_fails_or_leaves_keeps_the_task_open_in_every_state() {
    for r in [R::Failed, R::Left] {
        for b in [B::Ready, B::Asleep, B::Failed] {
            let v = out(b, r, false);
            assert!(!v.closes && v.warning.is_none());
            let d = v.decision.as_ref().expect("decision");
            assert!(d.task_input.is_none());
            assert_eq!(
                b.step(d.box_input.as_ref().expect("box input")),
                Ok(B::ReleaseFailed)
            );
        }
    }
}

#[test]
fn repeated_plain_close_failures_on_a_release_failed_box_stay_open() {
    for r in [R::Failed, R::Left] {
        let v = out(B::ReleaseFailed, r, false);
        assert!(!v.closes && v.warning.is_none());
        assert!(v.decision.is_none_or(|d| d.task_input.is_none()));
    }
}

#[test]
fn a_plain_close_with_a_driver_error_and_no_lease_rows_closes_with_a_warning() {
    for b in [B::Ready, B::Asleep, B::Failed, B::ReleaseFailed] {
        let v = out_rows(b, R::Failed, false, false);
        assert!(v.closes, "{b:?}");
        assert_eq!(
            apply(&v, TaskState::Working, b),
            (TaskState::Closed, B::ReleaseFailed)
        );
        let w = v.warning.expect("warning");
        assert_eq!(w.code, "box_release_failed");
        assert!(w.message.contains("Driver") && w.message.contains("manual release"));
    }
}

#[test]
fn a_plain_close_with_a_driver_error_and_lease_rows_left_stays_open() {
    for b in [B::Ready, B::Failed, B::ReleaseFailed] {
        let v = out_rows(b, R::Failed, false, true);
        assert!(!v.closes && v.warning.is_none(), "{b:?}");
    }
}

fn cause(v: ReleaseVerdict) -> String {
    match v.decision.expect("decision").box_input {
        Some(BoxInput::ReleaseErr { cause }) => cause,
        other => format!("{other:?}"),
    }
}

#[test]
fn the_left_cause_names_the_items_and_not_the_driver() {
    let c = cause(out(B::Ready, R::Left, false));
    assert_eq!(c, "the Box release left: root /srv/t7; process 4242");
    assert!(!c.contains("Driver"));
    assert!(cause(out(B::Ready, R::Failed, false)).contains("Driver"));
}

#[test]
fn a_forced_close_with_rows_left_closes_and_lists_them() {
    for b in [B::Ready, B::Asleep, B::Failed, B::ReleaseFailed] {
        let v = out(b, R::Left, true);
        assert!(v.closes);
        assert_eq!(
            apply(&v, TaskState::Working, b),
            (TaskState::Closed, B::ReleaseFailed)
        );
        let w = v.warning.expect("warning");
        assert_eq!(w.code, "box_release_failed");
        assert!(w.message.contains("root /srv/t7") && w.message.contains("process 4242"));
        assert!(w.message.contains("manual cleanup"));
        assert!(!w.message.contains("later release") && !w.message.contains("Driver"));
    }
}

#[test]
fn a_forced_close_after_a_driver_failure_closes_and_says_so() {
    let v = out(B::Failed, R::Failed, true);
    assert!(v.closes);
    assert_eq!(
        apply(&v, TaskState::SetupFailed, B::Failed),
        (TaskState::Closed, B::ReleaseFailed)
    );
    let w = v.warning.expect("warning");
    assert!(w.message.contains("Driver") && w.message.contains("manual cleanup"));
    assert!(!w.message.contains("later release") && !w.message.contains("RAWDRIVER"));
}

#[test]
fn box_with_only_uncommitted_changes_is_not_offered_a_bundle() {
    let f = CloseFacts {
        box_check: BoxCheck::Unsaved { ahead: 0, dirty: 3 },
        ..facts()
    };
    let text = close_gate(&f, false).expect_err("refused").to_string();
    for want in [
        "3 uncommitted file(s)",
        "`git push moor HEAD:refs/heads/moor/fix-bug`",
        "`moor run fix-bug --no-sync -- git status`",
        "`moor close fix-bug --force`",
    ] {
        assert!(text.contains(want), "{want}: {text}");
    }
    assert!(!text.contains("bundle"), "{text}");
}

#[test]
fn box_with_unpushed_commits_keeps_the_bundle_hint() {
    let f = CloseFacts {
        box_check: BoxCheck::Unsaved { ahead: 1, dirty: 3 },
        ..facts()
    };
    let text = close_gate(&f, false).expect_err("refused").to_string();
    assert!(text.contains("git bundle create"), "{text}");
}
