//! Tests for the close decider.

use super::{
    close_gate, release_outcome, BoxCheck, CloseFacts, Refusal, ReleaseResult, ReleaseVerdict,
};
use crate::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use crate::domain::boxes::BoxState;
use crate::domain::session_state::{SessionInput, SessionState};
use crate::name::BoxId;

use BoxState as B;
use ReleaseResult as R;

/// Clean facts for a working Box with a Ready lifecycle.
fn facts() -> CloseFacts {
    CloseFacts {
        box_id: BoxId::new(7),
        lifecycle: BoxLifecycle::Ready,
        session: SessionState::Working,
        box_state: Some(B::Ready),
        has_handle: true,
        name: "fix-bug".to_owned(),
        branch: Some("moor/fix-bug".to_owned()),
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
            name: "fix-bug".to_owned(),
            branch: "moor/fix-bug".to_owned(),
            ahead: 2,
            dirty: 3,
        }
    );
    let text = err.to_string();
    for want in [
        "2 unpushed commit(s)",
        "3 uncommitted file(s)",
        "not pushed",
        "`moor run fix-bug -- git push origin HEAD:refs/heads/moor/fix-bug`",
        "`moor run fix-bug -- git bundle create .moor/rescue.bundle HEAD --not --remotes`",
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
    assert!(text.contains("ssh timed out"), "{text}");
    assert!(text.contains("try the close again"), "{text}");
    assert!(text.contains("discard any work"), "{text}");
    assert!(!text.contains("--force"), "no CLI flag: {text}");
}

#[test]
fn box_whose_setup_failed_before_its_agent_launched_closes_unchecked() {
    let f = CloseFacts {
        lifecycle: BoxLifecycle::SetupFailed,
        session: SessionState::Starting,
        box_check: BoxCheck::Unknown("ssh timed out".to_owned()),
        ..facts()
    };
    let plan = close_gate(&f, false).expect("closes");
    assert!(plan.release_box);
}

#[test]
fn launched_box_whose_probe_cannot_run_is_still_refused() {
    for (lifecycle, session) in [
        (BoxLifecycle::Ready, SessionState::Working),
        (BoxLifecycle::Ready, SessionState::Starting),
        (BoxLifecycle::SetupFailed, SessionState::Working),
    ] {
        let f = CloseFacts {
            lifecycle,
            session,
            box_check: BoxCheck::Unknown("ssh timed out".to_owned()),
            ..facts()
        };
        assert!(
            matches!(close_gate(&f, false), Err(Refusal::BoxUnchecked { .. })),
            "{lifecycle:?} {session:?}"
        );
    }
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
fn clean_ready_box_is_released_and_closed() {
    let plan = close_gate(&facts(), false).ok();
    let plan = plan.expect("plan");
    assert!(plan.release_box && plan.close_workspace);
    assert_eq!(
        plan.decision_close.lifecycle_input,
        Some(BoxLifecycleInput::Close)
    );
    assert_eq!(plan.decision_close.session_input, Some(SessionInput::End));
}

#[test]
fn unsaved_work_is_refused_without_force() {
    for (ahead, dirty) in [(1, 0), (0, 1), (3, 2)] {
        let f = CloseFacts {
            box_check: BoxCheck::Unsaved { ahead, dirty },
            ..facts()
        };
        assert!(
            matches!(close_gate(&f, false), Err(Refusal::UnsavedInBox { .. })),
            "{ahead} {dirty}"
        );
        assert!(close_gate(&f, true).is_ok());
    }
}

#[test]
fn refusal_text_suggests_force_and_names_no_driver() {
    let text = Refusal::UnsavedInBox {
        name: "fix-bug".to_owned(),
        branch: "moor/fix-bug".to_owned(),
        ahead: 2,
        dirty: 1,
    }
    .to_string();
    assert!(text.contains("--force") && text.contains('2'), "{text}");
}

#[test]
fn closed_and_rejected_boxes_are_refused() {
    for lifecycle in [BoxLifecycle::Closed, BoxLifecycle::Rejected] {
        let f = CloseFacts {
            lifecycle,
            ..facts()
        };
        assert_eq!(close_gate(&f, true).err(), Some(Refusal::AlreadyClosed));
    }
}

#[test]
fn every_live_state_closes() {
    for lifecycle in BoxLifecycle::ALL.into_iter().filter(|l| l.is_live()) {
        for session in SessionState::ALL {
            let f = CloseFacts {
                lifecycle,
                session,
                ..facts()
            };
            assert!(close_gate(&f, false).is_ok(), "{lifecycle:?} {session:?}");
        }
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
fn apply(v: &ReleaseVerdict, life: BoxLifecycle, b: BoxState) -> (BoxLifecycle, BoxState) {
    let d = v.decision.as_ref().expect("decision");
    let l = d
        .lifecycle_input
        .as_ref()
        .map_or(Ok(life), |i| life.step(&[], i));
    let bx = d.box_input.as_ref().map_or(Ok(b), |i| b.step(i));
    (l.expect("lifecycle step"), bx.expect("box step"))
}

#[test]
fn deleted_and_already_gone_mark_the_box_deleted() {
    for r in [R::Deleted, R::AlreadyGone] {
        for b in [B::Ready, B::Asleep, B::Failed, B::ReleaseFailed] {
            let v = release_outcome(BoxId::new(7), b, r);
            assert!(v.closes && v.warning.is_none());
            assert_eq!(
                apply(&v, BoxLifecycle::Ready, b),
                (BoxLifecycle::Closed, B::Deleted)
            );
        }
    }
}

#[test]
fn failed_release_of_a_live_box_keeps_the_box_open() {
    for b in [B::Ready, B::Asleep] {
        let v = release_outcome(BoxId::new(7), b, R::Failed);
        assert!(!v.closes && v.decision.is_none() && v.warning.is_none());
    }
}

#[test]
fn failed_release_of_a_failed_box_closes_with_release_failed_and_warns() {
    let v = release_outcome(BoxId::new(7), B::Failed, R::Failed);
    assert!(v.closes);
    assert_eq!(
        apply(&v, BoxLifecycle::SetupFailed, B::Failed),
        (BoxLifecycle::Closed, B::ReleaseFailed)
    );
    let w = v.warning.expect("warning");
    assert_eq!(w.code, "box_release_failed");
    assert!(!w.message.contains("RAWDRIVER"));
    assert!(w.message.contains("not deleted") && w.message.contains("handle kept"));
}

#[test]
fn failed_release_of_a_release_failed_box_stays_release_failed() {
    let v = release_outcome(BoxId::new(7), B::ReleaseFailed, R::Failed);
    assert!(v.closes && v.warning.is_some());
    assert_eq!(
        apply(&v, BoxLifecycle::SetupFailed, B::ReleaseFailed),
        (BoxLifecycle::Closed, B::ReleaseFailed)
    );
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
        "`moor run fix-bug -- git push origin HEAD:refs/heads/moor/fix-bug`",
        "`moor run fix-bug -- git status`",
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

#[test]
fn every_refusal_maps_to_its_kind() {
    use crate::response::CloseRefusal as C;
    for (refusal, want) in [
        (Refusal::AlreadyClosed, C::AlreadyClosed),
        (
            Refusal::UnsavedInBox {
                name: "fix-bug".to_owned(),
                branch: "moor/fix-bug".to_owned(),
                ahead: 2,
                dirty: 1,
            },
            C::Unsaved {
                branch: "moor/fix-bug".to_owned(),
                ahead: 2,
                dirty: 1,
            },
        ),
        (
            Refusal::BoxUnchecked {
                reason: "ssh timed out".to_owned(),
            },
            C::Unchecked {
                reason: "ssh timed out".to_owned(),
            },
        ),
    ] {
        assert_eq!(C::from(&refusal), want);
    }
}
