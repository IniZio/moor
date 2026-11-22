//! The Box event a run's move writes when its owner should hear of it: a run that waits at an
//! approve step, and a run that ended `Failed`, `Incomplete`, `TimedOut` or `Refused`. Success writes
//! none, so delivered, declined and skipped runs stay silent.
//!
//! The event is written in the transaction of the compare-and-set that made the move, so a
//! move that changed nothing (a repeated tick, a restart, a lost race) writes nothing.

use moor_core::domain::event::{Event, WorkflowEvent};
use moor_core::gate::{wait, GateLine, Gates};
use moor_core::seam::clock::Ts;
use moor_core::workflow::{Plan, RunEnd, RunId, RunOutcome, RunState, StepKind, WorkflowId};
use moor_core::BoxId;
use moor_schema::tables::{BoxGateLines, Boxes, WorkflowRuns, Workflows};
use sea_query::{Expr, ExprTrait as _, Order, Query};

use super::corrupt;
use crate::store::apply::insert_event;
use crate::store::db::{Db, DbExt as _, OptionalExtension as _, Tx};
use crate::store::StoreError;

/// The workflow and repo of a run, and the Box and step it held before a move, which a refusal
/// drops from the row.
pub(super) struct Held {
    /// The workflow.
    workflow: WorkflowId,
    /// The repo column.
    repo: String,
    /// The Box column.
    box_id: Option<i64>,
    /// The step column.
    step: Option<String>,
}

/// Reads what run `run` holds, before its move; none when there is no such run.
pub(super) fn held(tx: &dyn Db, run: RunId) -> Result<Option<Held>, StoreError> {
    Ok(tx
        .row(
            Query::select()
                .columns([
                    WorkflowRuns::Workflow,
                    WorkflowRuns::Repo,
                    WorkflowRuns::Box,
                    WorkflowRuns::Step,
                ])
                .from(WorkflowRuns::Table)
                .and_where(Expr::col(WorkflowRuns::Id).eq(run.get())),
            |r| {
                Ok(Held {
                    workflow: WorkflowId::new(r.get(0)?),
                    repo: r.get(1)?,
                    box_id: r.get(2)?,
                    step: r.get(3)?,
                })
            },
        )
        .optional()?)
}

/// Writes the event for run `run`'s move to `to`, if it is one the owner hears of.
///
/// A refusal before any Box was recorded has no Box to carry an event, so it writes none.
pub(super) fn record(
    tx: &Tx<'_>,
    run: RunId,
    held: &Held,
    to: &RunState,
    at: Ts,
) -> Result<(), StoreError> {
    let (name, prompt) = workflow_of(tx, held.workflow)?;
    let run = run.get();
    let link = format!("/workflows/{name}/runs/{run}");
    let repo = held.repo.clone();
    let (box_id, event) = match to {
        RunState::Waiting {
            box_id, at: step, ..
        } => {
            let question = match gate_question(tx, box_id.get())? {
                Some(asked) => asked,
                None => question_of(tx, run, &prompt, step.as_str())?,
            };
            let event = WorkflowEvent::RunWaiting {
                workflow: name,
                repo,
                run,
                step: step.as_str().to_owned(),
                question,
                link,
            };
            (box_id.get(), event)
        }
        RunState::Done(end) => {
            let Some((box_id, outcome, step, reason)) = failure(end, held) else {
                return Ok(());
            };
            let event = WorkflowEvent::RunFailed {
                workflow: name,
                repo,
                run,
                outcome: outcome.to_owned(),
                step,
                reason,
                link,
            };
            (box_id, event)
        }
        RunState::Queued | RunState::Running { .. } => return Ok(()),
    };
    if !box_exists(tx, box_id)? {
        return Ok(());
    }
    insert_event(tx, BoxId::new(box_id), &Event::Workflow(event), at)?;
    Ok(())
}

/// The Box, outcome name, step and reason of an end the owner hears of; none for a delivery, a
/// decline, a cancel, a skip, and a refusal that never had a Box.
fn failure(end: &RunEnd, held: &Held) -> Option<(i64, &'static str, Option<String>, String)> {
    match end {
        RunEnd::Refused { detail } => {
            Some((held.box_id?, "refused", held.step.clone(), detail.clone()))
        }
        RunEnd::Ended {
            box_id, outcome, ..
        } => {
            let (kind, at, reason) = match outcome {
                RunOutcome::Failed { at, reason } => ("failed", at, reason.clone()),
                RunOutcome::Incomplete { at } => (
                    "incomplete",
                    at,
                    "the agent stopped without reporting the step, even after a reminder"
                        .to_owned(),
                ),
                RunOutcome::TimedOut { at } => (
                    "timed_out",
                    at,
                    "the run worked past its timeout".to_owned(),
                ),
                RunOutcome::Delivered { .. }
                | RunOutcome::Declined { .. }
                | RunOutcome::Cancelled { .. } => return None,
            };
            Some((box_id.get(), kind, Some(at.as_str().to_owned()), reason))
        }
        RunEnd::SkippedOverlap | RunEnd::SkippedFork { .. } | RunEnd::CancelledUnstarted { .. } => {
            None
        }
    }
}

/// The name and prompt of workflow `id`.
fn workflow_of(tx: &dyn Db, id: WorkflowId) -> Result<(String, String), StoreError> {
    Ok(tx.row(
        Query::select()
            .column(Workflows::Name)
            .column(Workflows::Prompt)
            .from(Workflows::Table)
            .and_where(Expr::col(Workflows::Id).eq(id.get())),
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?)
}

/// What the gate that holds Box `box_id` asks or says, when the Box has gates and one waits for a
/// person; none otherwise.
fn gate_question(tx: &dyn Db, box_id: i64) -> Result<Option<String>, StoreError> {
    let text: Option<Option<String>> = tx
        .row(
            Query::select()
                .column(Boxes::Gates)
                .from(Boxes::Table)
                .and_where(Expr::col(Boxes::Id).eq(box_id)),
            |r| r.get(0),
        )
        .optional()?;
    let Some(text) = text.flatten() else {
        return Ok(None);
    };
    let gates = Gates::from_json(&text).map_err(|e| corrupt("box gates", e))?;
    let lines = tx
        .rows(
            Query::select()
                .column(BoxGateLines::Entry)
                .from(BoxGateLines::Table)
                .and_where(Expr::col(BoxGateLines::Box).eq(box_id))
                .order_by(BoxGateLines::N, Order::Asc),
            |r| r.get::<_, String>(0),
        )?
        .map(|row| serde_json::from_str::<GateLine>(&row?).map_err(|e| corrupt("gate line", e)))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(wait(&gates, &lines).map(|w| w.question()))
}

/// What the approve step `step` of run `run`'s frozen plan asks.
fn question_of(tx: &dyn Db, run: i64, prompt: &str, step: &str) -> Result<String, StoreError> {
    let frozen: Option<Option<String>> = tx
        .row(
            Query::select()
                .column(WorkflowRuns::Plan)
                .from(WorkflowRuns::Table)
                .and_where(Expr::col(WorkflowRuns::Id).eq(run)),
            |r| r.get(0),
        )
        .optional()?;
    let plan = match frozen.flatten() {
        Some(json) => Plan::parse(&json, prompt).map_err(|e| {
            let why: Vec<String> = e.iter().map(ToString::to_string).collect();
            corrupt("run plan", why.join("; "))
        })?,
        None => Plan::single(prompt),
    };
    let asked = plan
        .steps()
        .find(|s| s.id.as_str() == step)
        .and_then(|s| match &s.kind {
            StepKind::Approve { question, .. } => Some(question.clone()),
            _ => None,
        });
    Ok(asked.unwrap_or_else(|| step.to_owned()))
}

/// Whether Box `id` has a row.
fn box_exists(tx: &dyn Db, id: i64) -> Result<bool, StoreError> {
    Ok(tx
        .row(
            Query::select()
                .column(Boxes::Id)
                .from(Boxes::Table)
                .and_where(Expr::col(Boxes::Id).eq(id)),
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .is_some())
}
