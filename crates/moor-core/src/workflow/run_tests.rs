//! Tests for plans, run states and history lines.
#![allow(
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cognitive_complexity,
    reason = "test assertions index JSON values and panic on a wrong shape"
)]

use super::{
    HistoryEntry, Plan, PromptHash, RunEnd, RunOutcome, RunPhase, RunState, StepId, StepKind,
    WorkflowError,
};
use crate::name::BoxId;
use crate::seam::clock::Ts;

#[test]
fn step_ids_are_short_lowercase_and_start_with_a_letter() {
    for ok in ["deliver", "a", "fix-2", &"a".repeat(32)] {
        assert!(StepId::parse(ok).is_ok(), "{ok}");
    }
    for bad in ["", "2fix", "-x", "Fix", "a_b", "a b", &"a".repeat(33)] {
        assert_eq!(
            StepId::parse(bad),
            Err(WorkflowError::StepId(bad.to_owned())),
            "{bad}"
        );
    }
}

#[test]
fn a_workflow_nobody_planned_has_one_deliver_step_made_from_its_prompt() {
    let plan = Plan::single("update the dependencies");
    assert_eq!(plan.first().id, Plan::single_id());
    assert_eq!(plan.steps().count(), 1);
    assert!(!plan.edited);
    assert_eq!(plan.made_from, PromptHash::of("update the dependencies"));
    assert_ne!(plan.made_from, PromptHash::of("something else"));
    let StepKind::Deliver { instructions, .. } = &plan.first().kind else {
        panic!("the single plan delivers");
    };
    assert_eq!(instructions, "update the dependencies");
}

fn ended(outcome: RunOutcome) -> RunState {
    RunState::Done(RunEnd::Ended {
        box_id: BoxId::new(3),
        ended_at: Ts::from_millis(9),
        outcome,
    })
}

fn running() -> RunState {
    RunState::Running {
        box_id: BoxId::new(3),
        at: Plan::single_id(),
        try_n: 1,
    }
}

#[test]
fn a_run_state_keeps_its_phase_apart_from_its_outcome() {
    let at = Plan::single_id();
    let waiting = RunState::Waiting {
        box_id: BoxId::new(3),
        at: at.clone(),
        try_n: 1,
        since: Ts::from_millis(5),
    };
    let refused = RunState::Done(RunEnd::Refused {
        detail: "full".to_owned(),
    });
    let delivered = ended(RunOutcome::Delivered {
        result: "done".to_owned(),
        verdict: None,
    });
    let timed_out = ended(RunOutcome::TimedOut { at: at.clone() });
    let skipped = RunState::Done(RunEnd::SkippedOverlap);
    let cases = [
        (RunState::Queued, RunPhase::Queued, "starting", None, None),
        (running(), RunPhase::Running, "started", Some(3), Some(&at)),
        (waiting, RunPhase::Waiting, "waiting", Some(3), Some(&at)),
        (delivered, RunPhase::Done, "finished", Some(3), None),
        (timed_out, RunPhase::Done, "timed_out", Some(3), Some(&at)),
        (refused, RunPhase::Done, "refused", None, None),
        (skipped, RunPhase::Done, "skipped_overlap", None, None),
    ];
    for (state, phase, tag, box_id, step) in cases {
        assert_eq!(state.phase(), phase, "{state:?}");
        assert_eq!(state.tag(), tag, "{state:?}");
        assert_eq!(state.box_id().map(BoxId::get), box_id, "{state:?}");
        assert_eq!(state.step(), step, "{state:?}");
    }
}

#[test]
fn history_lines_are_tagged_json_and_say_where_the_run_was() {
    let refused = RunState::Done(RunEnd::Refused {
        detail: "full".to_owned(),
    });
    let delivered = ended(RunOutcome::Delivered {
        result: "done".to_owned(),
        verdict: None,
    });
    let lines = [
        (RunState::Queued, RunPhase::Queued, r#"{"kind":"queued"}"#),
        (
            running(),
            RunPhase::Queued,
            r#"{"kind":"entered","step":"deliver","try_n":1}"#,
        ),
        (
            RunState::Done(RunEnd::SkippedOverlap),
            RunPhase::Queued,
            r#"{"kind":"skipped"}"#,
        ),
        (
            refused,
            RunPhase::Queued,
            r#"{"kind":"refused","phase_before":"queued","detail":"full"}"#,
        ),
        (
            delivered,
            RunPhase::Running,
            r#"{"kind":"ended","phase_before":"running","outcome":{"outcome":"delivered","result":"done"}}"#,
        ),
    ];
    for (to, from, json) in lines {
        let entry = HistoryEntry::moving(from, &to);
        assert_eq!(serde_json::to_string(&entry).unwrap(), json);
        assert_eq!(serde_json::from_str::<HistoryEntry>(json).unwrap(), entry);
    }
}
