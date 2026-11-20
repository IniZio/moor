//! `workflow-answer`: the owner's yes or no for the approve step a run waits at.
//!
//! The answer moves the run in one transaction with its lines, compare-and-set on the run waiting
//! at exactly that step and try, so two answers, or an answer and the week's expiry, apply once.
//! The move is committed before the agent is told, so only the answer that wins steers; a crash
//! or a failed steer after it leaves the run at the step with no `Sent` line, and the tick
//! reminds the agent of it. A steer to an asleep Box wakes it.

use moor_core::decide::workflow::{answer_move, Move};
use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_core::view::workflow::WorkflowAnswerValue;
use moor_core::workflow::{
    Answer, HistoryEntry, Plan, RunId, RunOutcome, RunState, Step, StepId, StepKind, WorkflowName,
    WorkflowRun, NOTE_MAX,
};
use moor_core::{BoxId, ErrorKind};

use super::advance::send;
use super::facts::history_of;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{BoxRow, Store};

/// A refusal the caller can act on.
fn refused(why: impl std::fmt::Display) -> CommandError {
    CommandError::new(ErrorKind::Refused, why.to_string())
}

/// Why an answer found the run gone from the step it waited at.
const MOVED: &str = "the run moved while you answered: it was answered already, or it ended";

/// Whether the last step `history` entered is `at`, the step and try the run waits at.
fn entered_last(history: &[HistoryEntry], at: (&StepId, u8)) -> bool {
    let last = history.iter().rev().find_map(|e| match e {
        HistoryEntry::Entered { step, try_n } => Some((step, *try_n)),
        _ => None,
    });
    last == Some(at)
}

/// A refusal for input the caller can correct.
fn usage(why: impl std::fmt::Display) -> CommandError {
    CommandError::new(ErrorKind::Usage, why.to_string())
}

/// A run waiting at an approve step, and the answer it is given.
struct Asked<'a> {
    /// The controller context.
    cx: &'a Cx,
    /// The store.
    store: &'a Store,
    /// The run.
    run: &'a WorkflowRun,
    /// The run's Box.
    box_id: BoxId,
    /// The approve step and its try.
    at: (&'a StepId, u8),
    /// The run's frozen plan.
    plan: &'a Plan,
    /// The approve step.
    ask: &'a Step,
    /// Who answers, how, and with what note.
    given: (UserId, Answer, Option<&'a str>),
    /// Now.
    now: Ts,
}

/// Answers the run of `user`'s workflow `workflow` that waits at an approve step.
pub(crate) fn run(
    cx: &Cx,
    store: &Store,
    user: UserId,
    (workflow, answer, note): (&str, Answer, Option<&str>),
) -> Result<WorkflowAnswerValue, CommandError> {
    let name = WorkflowName::parse(workflow).map_err(usage)?;
    let note = note.map(str::trim).filter(|n| !n.is_empty());
    if note.is_some_and(|n| n.len() > NOTE_MAX) {
        return Err(usage(format!("the note is at most {NOTE_MAX} bytes")));
    }
    // Only the owner's own workflows are found, so nobody else can answer for them.
    let Some(w) = store.workflow(user, &name)? else {
        return Err(refused(format!("no workflow named {name}")));
    };
    let waiting = store.open_workflow_runs(w.id)?.into_iter().find_map(|r| {
        let RunState::Waiting {
            box_id,
            ref at,
            try_n,
            ..
        } = r.state
        else {
            return None;
        };
        let at = at.clone();
        Some((box_id, at, try_n, r))
    });
    let Some((box_id, at, try_n, run)) = waiting else {
        return Err(refused(format!(
            "no run of {name} is waiting for an answer"
        )));
    };
    let plan = store.run_plan(run.id, &w.prompt)?;
    #[cfg(test)]
    tests_hook::after_read(cx);
    let (history, _) = history_of(store, run.id, (&at, try_n))?;
    // An answer that read the run waiting may read the lines of the answer that moved it: refuse
    // unless the run's last entry is still the wait, so no move is worked out from a later step.
    if !entered_last(&history, (&at, try_n)) {
        return Err(refused(MOVED));
    }
    let Some(ask) = plan
        .step(&at)
        .filter(|s| matches!(s.kind, StepKind::Approve { .. }))
    else {
        return Err(refused(format!(
            "step {at} of the plan is not an approve step"
        )));
    };
    let asked = Asked {
        cx,
        store,
        run: &run,
        box_id,
        at: (&at, try_n),
        plan: &plan,
        ask,
        given: (user, answer, note),
        now: cx.clock().now(),
    };
    let next = asked.apply(answer_move(&plan, &history, answer))?;
    Ok(WorkflowAnswerValue {
        workflow: name.to_string(),
        next,
    })
}

impl Asked<'_> {
    /// The run's key.
    const fn key(&self) -> RunId {
        self.run.id
    }

    /// The line for the answer itself.
    fn answered(&self) -> HistoryEntry {
        let (by, answer, note) = self.given;
        HistoryEntry::Answered {
            step: self.at.0.clone(),
            by,
            answer,
            note: note.map(str::to_owned),
        }
    }

    /// Makes `m`, the move the answer decided; what the run did, in a sentence.
    fn apply(&self, m: Move) -> Result<String, CommandError> {
        let (done, said) = match m {
            Move::Enter {
                step,
                try_n,
                exhausted,
            } => {
                let told = self.go_on((step.clone(), try_n), exhausted)?;
                (
                    told.is_some(),
                    format!("the run went on to step {step}{}", told.unwrap_or_default()),
                )
            }
            Move::Fail {
                at,
                reason,
                exhausted,
            } => {
                let lines = self.lines_with(exhausted);
                let outcome = RunOutcome::Failed { at, reason };
                (
                    self.end(&lines, outcome)?,
                    "the run ended failed".to_owned(),
                )
            }
            Move::Declined { at } => (
                self.end(&[self.answered()], RunOutcome::Declined { at })?,
                "the run ended declined".to_owned(),
            ),
            Move::Wait
            | Move::Remind
            | Move::RunCheck
            | Move::Ask
            | Move::Delivered
            | Move::Incomplete { .. } => return Err(refused("the run could not move on")),
        };
        if done {
            Ok(said)
        } else {
            Err(refused(MOVED))
        }
    }

    /// The answer's line, then a line for each step passed over for lack of tries.
    fn lines_with(&self, exhausted: Vec<StepId>) -> Vec<HistoryEntry> {
        let mut lines = vec![self.answered()];
        lines.extend(
            exhausted
                .into_iter()
                .map(|step| HistoryEntry::Exhausted { step }),
        );
        lines
    }

    /// Ends the run with `outcome`; false when it no longer waits at the step.
    fn end(&self, lines: &[HistoryEntry], outcome: RunOutcome) -> Result<bool, CommandError> {
        Ok(self.store.end_waiting_run(
            self.key(),
            self.at,
            (self.box_id, outcome),
            (lines, self.now),
        )?)
    }

    /// Sends the run on to `step`, for its `try_n`th entry: waiting again at an approve step,
    /// else running there. The move is committed first; when the step is the agent's to do it is
    /// told after, and a failed steer is said in the answer's reply, not undone, for the tick to
    /// remind the agent. None when the run no longer waits at the step, with nothing steered;
    /// else what to add to the reply.
    fn go_on(
        &self,
        (step, try_n): (StepId, u8),
        exhausted: Vec<StepId>,
    ) -> Result<Option<String>, CommandError> {
        let Some(next) = self.plan.step(&step) else {
            return Err(refused("the run's plan has no such step"));
        };
        let mut lines = self.lines_with(exhausted);
        lines.push(HistoryEntry::Entered {
            step: step.clone(),
            try_n,
        });
        let box_id = self.box_id;
        let to = if matches!(next.kind, StepKind::Approve { .. }) {
            RunState::Waiting {
                box_id,
                at: step.clone(),
                try_n,
                since: self.now,
            }
        } else {
            RunState::Running {
                box_id,
                at: step.clone(),
                try_n,
            }
        };
        let tells = matches!(next.kind, StepKind::Work { .. } | StepKind::Deliver { .. });
        if tells {
            // Nothing is recorded for a Box that is already closed, so the owner can answer again.
            live(self.store, box_id)?;
        }
        if !self
            .store
            .answer_workflow_run(self.key(), self.at, &to, (&lines, self.now))?
        {
            return Ok(None);
        }
        if !tells {
            return Ok(Some(String::new()));
        }
        let (_, answer, note) = self.given;
        let text = Plan::answer_steer(self.ask, (answer, note), next);
        let sent = live(self.store, box_id).and_then(|row| {
            send(self.cx, &row, &text).map_err(|why| refused(format!("could not send it ({why})")))
        });
        if let Err(why) = sent {
            return Ok(Some(format!(
                ", but the agent could not be told ({}); the run reminds it",
                why.error
            )));
        }
        let line = HistoryEntry::Sent {
            step: step.clone(),
            try_n,
        };
        let at = (&step, try_n);
        self.store
            .step_workflow_run(self.key(), (at, at), (&[line], self.now))?;
        Ok(Some(String::new()))
    }
}

/// The Box `id` when it is live; a closed or missing Box is a refusal.
fn live(store: &Store, id: BoxId) -> Result<BoxRow, CommandError> {
    store
        .box_row(id)?
        .filter(|r| r.lifecycle.is_live())
        .ok_or_else(|| refused("the run's Box is closed, so there is no agent to tell"))
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
