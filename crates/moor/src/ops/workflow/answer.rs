//! `workflow-answer`: the owner's yes or no for the approve step a run waits at.
//!
//! The answer moves the run in one transaction with its lines, compare-and-set on the run waiting
//! at exactly that step and try, so two answers, or an answer and the week's expiry, apply once.
//! The next step's instructions go out as a steer first and the move is recorded after, as the
//! tick does, so a crash in between sends the step twice and never skips it. A steer to an asleep
//! Box wakes it.

use moor_core::decide::workflow::{answer_move, Move};
use moor_core::name::ForgeRepo;
use moor_core::user::UserId;
use moor_core::view::workflow::WorkflowAnswerValue;
use moor_core::workflow::{
    Answer, HistoryEntry, Plan, RunOutcome, RunState, StepKind, WorkflowName, NOTE_MAX,
};
use moor_core::{BoxId, ErrorKind};

use super::advance::send;
use super::facts::history_of;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::Store;

/// A refusal the caller can act on.
fn refused(why: impl std::fmt::Display) -> CommandError {
    CommandError::new(ErrorKind::Refused, why.to_string())
}

/// A refusal for input the caller can correct.
fn usage(why: impl std::fmt::Display) -> CommandError {
    CommandError::new(ErrorKind::Usage, why.to_string())
}

/// Answers the run of `user`'s workflow `workflow` for `repo` that waits at an approve step.
pub(crate) fn run(
    cx: &Cx,
    store: &Store,
    user: UserId,
    (workflow, repo, answer, note): (&str, &str, Answer, Option<&str>),
) -> Result<WorkflowAnswerValue, CommandError> {
    let name = WorkflowName::parse(workflow).map_err(usage)?;
    let repo = ForgeRepo::parse(repo).map_err(|_| usage("repo must be OWNER/NAME"))?;
    let note = note.map(str::trim).filter(|n| !n.is_empty());
    if note.is_some_and(|n| n.len() > NOTE_MAX) {
        return Err(usage(format!("the note is at most {NOTE_MAX} bytes")));
    }
    // Only the owner's own workflows are found, so nobody else can answer for them.
    let Some(w) = store.workflow(user, &name)? else {
        return Err(refused(format!("no workflow named {name}")));
    };
    let run = store
        .open_workflow_runs(w.id)?
        .into_iter()
        .find(|r| r.repo == repo && matches!(r.state, RunState::Waiting { .. }));
    let Some(run) = run else {
        return Err(refused(format!(
            "no run of {name} for {repo} is waiting for an answer"
        )));
    };
    let RunState::Waiting {
        box_id,
        ref at,
        try_n,
        ..
    } = run.state
    else {
        return Err(refused("the run is not waiting"));
    };
    let plan = store.run_plan(run.id, &w.prompt)?;
    let (history, _) = history_of(store, run.id, (at, try_n))?;
    #[cfg(test)]
    tests_hook::after_read(cx);
    let Some(ask) = plan.step(at).filter(|s| matches!(s.kind, StepKind::Approve { .. })) else {
        return Err(refused(format!("step {at} of the plan is not an approve step")));
    };
    let now = cx.clock().now();
    let mut lines = vec![HistoryEntry::Answered {
        step: at.clone(),
        by: user,
        answer,
        note: note.map(str::to_owned),
    }];
    let key = (w.id, &run.repo, run.due_at);
    let on = (at, try_n);
    let moved = match answer_move(&plan, &history, answer) {
        Move::Enter {
            step,
            try_n: next_try,
            exhausted,
        } => {
            let Some(next) = plan.step(&step) else {
                return Err(refused("the run's plan has no such step"));
            };
            lines.extend(
                exhausted
                    .into_iter()
                    .map(|step| HistoryEntry::Exhausted { step }),
            );
            lines.push(HistoryEntry::Entered {
                step: step.clone(),
                try_n: next_try,
            });
            let to = if matches!(next.kind, StepKind::Approve { .. }) {
                RunState::Waiting {
                    box_id,
                    at: step.clone(),
                    try_n: next_try,
                    since: now,
                }
            } else {
                RunState::Running {
                    box_id,
                    at: step.clone(),
                    try_n: next_try,
                }
            };
            if matches!(next.kind, StepKind::Work { .. } | StepKind::Deliver { .. }) {
                let text = Plan::answer_steer(ask, (answer, note), next);
                tell(cx, store, box_id, &text)?;
                lines.push(HistoryEntry::Sent {
                    step: step.clone(),
                    try_n: next_try,
                });
            }
            let done = store.answer_workflow_run(key, on, &to, (&lines, now))?;
            (done, format!("the run went on to step {step}"))
        }
        Move::Fail {
            at: failed_at,
            reason,
            exhausted,
        } => {
            lines.extend(
                exhausted
                    .into_iter()
                    .map(|step| HistoryEntry::Exhausted { step }),
            );
            let outcome = RunOutcome::Failed {
                at: failed_at,
                reason,
            };
            let done = store.end_waiting_run(key, on, (box_id, outcome), (&lines, now))?;
            (done, "the run ended failed".to_owned())
        }
        Move::Declined { at: declined } => {
            let outcome = RunOutcome::Declined { at: declined };
            let done = store.end_waiting_run(key, on, (box_id, outcome), (&lines, now))?;
            (done, "the run ended declined".to_owned())
        }
        Move::Wait
        | Move::Remind
        | Move::RunCheck
        | Move::Ask
        | Move::Delivered
        | Move::Incomplete { .. } => return Err(refused("the run could not move on")),
    };
    if !moved.0 {
        return Err(refused(
            "the run moved while you answered: it was answered already, or it ended",
        ));
    }
    Ok(WorkflowAnswerValue {
        workflow: name.to_string(),
        repo: repo.to_string(),
        next: moved.1,
    })
}

/// Steers the agent in Box `id` with `text`; a closed or missing Box is a refusal that records
/// nothing, so the owner can answer again once the Box is back.
fn tell(cx: &Cx, store: &Store, id: BoxId, text: &str) -> Result<(), CommandError> {
    let row = store
        .box_row(id)?
        .filter(|r| r.lifecycle.is_live())
        .ok_or_else(|| refused("the run's Box is closed, so there is no agent to tell"))?;
    send(cx, &row, text).map_err(|why| {
        refused(format!(
            "could not send the next step to the Box ({why}); nothing is recorded, answer again"
        ))
    })
}

/// A test seam between an answer's reading of the run and its move.
#[cfg(test)]
pub(super) mod tests_hook {
    use std::cell::RefCell;

    use crate::cx::Cx;

    type Hook = Box<dyn FnOnce(&Cx)>;

    thread_local! {
        static AFTER_READ: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    /// Runs `f` once, after the next reading of a run by an answer on this thread.
    pub(in crate::ops::workflow) fn once_after_read(f: impl FnOnce(&Cx) + 'static) {
        AFTER_READ.with(|h| *h.borrow_mut() = Some(Box::new(f)));
    }

    pub(super) fn after_read(cx: &Cx) {
        if let Some(f) = AFTER_READ.with(|h| h.borrow_mut().take()) {
            f(cx);
        }
    }
}
