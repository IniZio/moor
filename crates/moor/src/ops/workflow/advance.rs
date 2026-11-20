//! Settling the started runs of a workflow from where each stands: a quiet agent at a work step
//! is sent on to the next step, an ended one is sent its step again once, a check step runs its
//! command and follows its exit, and a deliver step delivers. An approve step sets the run
//! waiting for its owner. A Box that is gone ends the run, and one no agent can run in refuses
//! it.

use moor_core::decide::workflow::{
    advance, quiet_since, run_verdict, waited_millis, Event, Move, RunBoxFacts, RunVerdict,
};
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::session_state::SessionState;
use moor_core::request::BoxTarget;
use moor_core::seam::clock::Ts;
use moor_core::workflow::{
    CheckSays, HistoryEntry, Plan, RunOutcome, RunState, Step, StepId, StepKind, Workflow,
    WorkflowRun,
};
use moor_core::{BoxId, SteerReq};

use super::check::{CheckJob, Flight, CHECK_LIMIT};
use super::facts::history_of;
use super::settle::{box_facts, broken_detail, fail_death_capped};
use crate::cx::Cx;
use crate::ops::steer;
use crate::store::{BoxRow, Store, StoreError};

/// Settles each started run of `w` whose Box is over or quiet: sent on to its next step,
/// delivered, failed, or refused when no agent can run in it until a person acts. A check that
/// is due goes to `checks`, which must not wait for it. The Box stays open for inspection and
/// nothing retries it.
pub(super) fn finish_runs(
    cx: &Cx,
    store: &Store,
    w: &Workflow,
    now: Ts,
    (checks, out): (&dyn Fn(CheckJob), &mut Vec<String>),
) -> Result<(), StoreError> {
    for run in store.open_workflow_runs(w.id)? {
        let RunState::Running {
            box_id,
            ref at,
            try_n,
        } = run.state
        else {
            if let RunState::Waiting {
                box_id,
                ref at,
                try_n,
                ..
            } = run.state
            {
                super::wait::lose_waiting(store, &run, (box_id, at, try_n), now)?;
            }
            continue;
        };
        let row = store.box_row(box_id)?;
        let facts = row.as_ref().map(|r| box_facts(store, r)).transpose()?;
        let verdict = run_verdict(facts.as_ref(), now);
        if verdict == RunVerdict::Broken {
            if let Some(row) = &row {
                let key = run.id;
                store.refuse_run(key, (&broken_detail(store, row), false), now)?;
            }
            continue;
        }
        if verdict == RunVerdict::DeathCapped {
            fail_death_capped(store, &run, (box_id, at, try_n), now)?;
            continue;
        }
        let plan = store.run_plan(run.id, &w.prompt)?;
        let at_check = matches!(plan.step(at).map(|s| &s.kind), Some(StepKind::Check { .. }));
        // A check runs as soon as the agent is idle, so an agent that only just stopped is
        // quiet enough for it. An approve step is the owner's to answer, so the run waits at
        // once. A step the agent works on waits for the longer quiet.
        let at_approve = matches!(
            plan.step(at).map(|s| &s.kind),
            Some(StepKind::Approve { .. })
        );
        if verdict == RunVerdict::Running && !at_check && !at_approve {
            continue;
        }
        let q = Quiet {
            cx,
            store,
            w,
            run: &run,
            row: row.as_ref(),
            at: (at, try_n),
            now,
        };
        q.settle(&plan, box_id, (checks, out))?;
    }
    Ok(())
}

/// A check of the run is already running.
struct CheckRunning;

/// Whether `row` is a Box that can still be steered.
fn open(row: Option<&BoxRow>) -> Option<&BoxRow> {
    row.filter(|r| !matches!(r.lifecycle, BoxLifecycle::Closed | BoxLifecycle::Rejected))
}

/// A started run with what the tick knows of it.
struct Quiet<'a> {
    /// The controller context.
    cx: &'a Cx,
    /// The store.
    store: &'a Store,
    /// The workflow.
    w: &'a Workflow,
    /// The run.
    run: &'a WorkflowRun,
    /// The run's Box, if it still exists.
    row: Option<&'a BoxRow>,
    /// The step the run is at, and its try.
    at: (&'a StepId, u8),
    /// Now.
    now: Ts,
}

/// Whether the agent of a Box with `facts` is quiet enough for a check to run: idle since the
/// last steer, or ended, in a Box that is up or asleep.
fn quiet_for_check(facts: &RunBoxFacts, steered: Option<Ts>, now: Ts) -> bool {
    if !matches!(facts.lifecycle, BoxLifecycle::Ready | BoxLifecycle::Asleep) {
        return false;
    }
    match facts.session {
        SessionState::Ended => true,
        SessionState::Idle => steered.is_none_or(|t| quiet_since(facts, t, now)),
        SessionState::Starting | SessionState::Working => false,
    }
}

impl Quiet<'_> {
    /// The run's history lines, and when the agent was last told something: a reading from
    /// before that is still the quiet of the step before. A run from before history kept an
    /// `Entered` line reads as entered at its pointer.
    fn history(&self) -> Result<(Vec<HistoryEntry>, Option<Ts>), StoreError> {
        history_of(self.store, self.run.id, (self.at.0, self.at.1))
    }

    /// The claim a tick at a check step holds while it settles: an error when a check of the
    /// run is already running, none at a step that is no check.
    fn claim_for(&self, at_check: bool) -> Result<Option<Flight>, CheckRunning> {
        if !at_check {
            return Ok(None);
        }
        Flight::claim(self.cx, self.run.id)
            .map(Some)
            .ok_or(CheckRunning)
    }

    /// What the run's standing means now, and the move it makes.
    fn settle(
        &self,
        plan: &Plan,
        box_id: BoxId,
        (checks, out): (&dyn Fn(CheckJob), &mut Vec<String>),
    ) -> Result<(), StoreError> {
        let step = plan.step(self.at.0);
        let at_check = matches!(step.map(|s| &s.kind), Some(StepKind::Check { .. }));
        // The claim comes before the history is read: a check thread writes its `Checked` line
        // and only then drops its flight, so a tick that holds the claim reads every line a
        // finished check wrote. Claiming after the read could start the same check twice.
        let Ok(flight) = self.claim_for(at_check) else {
            return Ok(());
        };
        let (history, steered) = self.history()?;
        #[cfg(test)]
        tests_hook::after_history(self.cx);
        let facts = open(self.row)
            .map(|r| box_facts(self.store, r))
            .transpose()?;
        let event = match &facts {
            Some(f) if f.session == SessionState::Ended => Event::Ended,
            _ => Event::Idle,
        };
        let m = advance(plan, &history, event);
        if let Some(f) = &facts {
            let waits = if at_check {
                m == Move::RunCheck && !quiet_for_check(f, steered, self.now)
            } else {
                steered.is_some_and(|sent| !quiet_since(f, sent, self.now))
            };
            if waits {
                return Ok(());
            }
        }
        let end = |outcome| self.end(box_id, outcome);
        match m {
            Move::Wait | Move::Declined { .. } => Ok(()),
            Move::Ask => self.ask(box_id),
            Move::Delivered => end(RunOutcome::Delivered {
                result: String::new(),
            }),
            Move::Remind | Move::RunCheck | Move::Enter { .. } | Move::Fail { .. }
                if open(self.row).is_none() =>
            {
                end(RunOutcome::Incomplete {
                    at: self.at.0.clone(),
                })
            }
            Move::Incomplete { at } => end(RunOutcome::Incomplete { at }),
            Move::Fail { at, reason, .. } => end(RunOutcome::Failed { at, reason }),
            Move::RunCheck => flight.map_or(Ok(()), |flight| {
                self.start_check(plan, box_id, (checks, flight))
            }),
            Move::Remind => {
                let Some(step) = step else { return Ok(()) };
                self.remind(step, out)
            }
            Move::Enter {
                step,
                try_n,
                exhausted,
            } => self.enter(plan, &history, (step, try_n), exhausted, out),
        }
    }

    /// Ends the run at the step and try this tick read it at; nothing changes when a report or an
    /// expiry moved it first.
    fn end(&self, box_id: BoxId, outcome: RunOutcome) -> Result<(), StoreError> {
        let key = self.run.id;
        self.store
            .end_workflow_run(key, self.at, (box_id, outcome), (&[], self.now))
            .map(|_| ())
    }

    /// Sets the run, which began at an approve step, waiting for its owner.
    fn ask(&self, box_id: BoxId) -> Result<(), StoreError> {
        let key = self.run.id;
        let at = (self.at.0, self.at.1);
        // The step is entered again as the wait begins, so a wait is counted from here.
        let line = HistoryEntry::Entered {
            step: at.0.clone(),
            try_n: at.1,
        };
        self.store
            .wait_workflow_run(key, (at, at), box_id, (&[line], self.now))
            .map(|_| ())
    }

    /// Hands the check at the pointer to `checks`, with a deadline of the smaller of the run's
    /// remaining timeout and [`CHECK_LIMIT`], under the `flight` the tick claimed.
    fn start_check(
        &self,
        plan: &Plan,
        box_id: BoxId,
        (checks, flight): (&dyn Fn(CheckJob), Flight),
    ) -> Result<(), StoreError> {
        let Some(StepKind::Check { command, .. }) = plan.step(self.at.0).map(|s| &s.kind) else {
            return Ok(());
        };
        let entries = self.store.run_entries(self.run.id)?;
        let spent = self.store.first_launch_at(box_id)?.map_or(0, |began| {
            let waited = waited_millis(&entries, began);
            self.now
                .as_millis()
                .saturating_sub(began.as_millis())
                .saturating_sub(waited)
                .max(0)
        });
        let left_secs = self.w.timeout.millis().saturating_sub(spent) / 1000;
        let limit = i64::try_from(CHECK_LIMIT.as_secs()).unwrap_or(i64::MAX);
        let timeout_secs = u32::try_from(left_secs.clamp(1, limit)).unwrap_or(1);
        checks(CheckJob {
            run: self.run.id,
            box_id,
            at: (self.at.0.clone(), self.at.1),
            command: command.clone(),
            timeout_secs,
            _flight: flight,
        });
        Ok(())
    }

    /// Sends the current step again and records the reminder.
    fn remind(&self, step: &Step, out: &mut Vec<String>) -> Result<(), StoreError> {
        let Some(row) = open(self.row) else {
            return Ok(());
        };
        if let Err(why) = send(self.cx, row, &Plan::remind_steer(step)) {
            out.push(format!(
                "workflow {}: could not remind {} of step {}: {why}",
                self.w.name,
                row.name.as_str(),
                step.id
            ));
            return Ok(());
        }
        let line = HistoryEntry::Reminded {
            step: self.at.0.clone(),
            try_n: self.at.1,
        };
        self.record((self.at.0, self.at.1), &[line])
    }

    /// Sends the run on to `step`, for its `try_n`th entry, and records the move. A step that is
    /// a check is not sent: the agent is quiet and moor runs the check itself.
    fn enter(
        &self,
        plan: &Plan,
        history: &[HistoryEntry],
        (step, try_n): (StepId, u8),
        exhausted: Vec<StepId>,
        out: &mut Vec<String>,
    ) -> Result<(), StoreError> {
        let (Some(row), Some(next)) = (open(self.row), plan.step(&step)) else {
            return Ok(());
        };
        let mut lines: Vec<HistoryEntry> = exhausted
            .into_iter()
            .map(|step| HistoryEntry::Exhausted { step })
            .collect();
        lines.push(HistoryEntry::Entered {
            step: step.clone(),
            try_n,
        });
        let key = self.run.id;
        if matches!(next.kind, StepKind::Approve { .. }) {
            return self
                .store
                .wait_workflow_run(key, (self.at, (&step, try_n)), row.id, (&lines, self.now))
                .map(|_| ());
        }
        if !matches!(next.kind, StepKind::Check { .. }) {
            // Steer first and record after, so a crash in between sends the step twice and
            // never skips it.
            if let Err(why) = send(self.cx, row, &self.steer_text(plan, history, next)) {
                out.push(format!(
                    "workflow {}: could not send step {step} to {}: {why}",
                    self.w.name,
                    row.name.as_str()
                ));
                return Ok(());
            }
            lines.push(HistoryEntry::Sent {
                step: step.clone(),
                try_n,
            });
        }
        self.store
            .step_workflow_run(key, (self.at, (&step, try_n)), (&lines, self.now))
            .map(|_| ())
    }

    /// What the agent is told on entering `next`: after a check, what the check said.
    fn steer_text(&self, plan: &Plan, history: &[HistoryEntry], next: &Step) -> String {
        let Some(check) = plan
            .step(self.at.0)
            .filter(|s| matches!(s.kind, StepKind::Check { .. }))
        else {
            return Plan::next_step_steer(next);
        };
        let tried = history
            .iter()
            .rposition(|e| matches!(e, HistoryEntry::Entered { .. }))
            .and_then(|i| history.get(i + 1..))
            .unwrap_or_default();
        let result = tried.iter().rev().find_map(|e| match e {
            HistoryEntry::Checked { exit, tail, .. } => Some((*exit, tail.as_str())),
            _ => None,
        });
        let says = match result {
            Some((0, tail)) => CheckSays::Passed { tail },
            Some((exit, tail)) => CheckSays::Failed { exit, tail },
            None => CheckSays::Lost,
        };
        Plan::after_check_steer(check, &says, next)
    }

    /// Appends `lines` to the run's history without moving its pointer off `at`.
    fn record(&self, at: (&StepId, u8), lines: &[HistoryEntry]) -> Result<(), StoreError> {
        let key = self.run.id;
        self.store
            .step_workflow_run(key, (at, at), (lines, self.now))
            .map(|_| ())
    }
}

/// Steers the agent of `row` with `text`, as its owner does.
pub(super) fn send(cx: &Cx, row: &BoxRow, text: &str) -> Result<(), String> {
    let req = SteerReq {
        target: BoxTarget::Id(row.id),
        text: Some(text.to_owned()),
        menu: None,
        interrupt: false,
        ask: None,
    };
    cx.for_box(row.user, || steer::run(cx, &req))
        .map(|_| ())
        .map_err(|e| e.error.to_string())
}

#[cfg(test)]
pub(super) use super::wait::tests_hook;
