//! The Box event a run's move writes when its owner should hear of it: a run that waits at an
//! approve step, and a run that ended `Failed`, `Incomplete`, `TimedOut` or `Refused`. Success writes
//! none, so delivered, declined and skipped runs stay silent.
//!
//! The event is written in the transaction of the compare-and-set that made the move, so a
//! move that changed nothing (a repeated tick, a restart, a lost race) writes nothing.

use moor_core::domain::event::{Event, WorkflowEvent};
use moor_core::name::ForgeRepo;
use moor_core::seam::clock::Ts;
use moor_core::workflow::{Plan, RunEnd, RunOutcome, RunState, StepKind, WorkflowId};
use moor_core::BoxId;
use moor_schema::tables::{Boxes, WorkflowRuns, Workflows};
use sea_query::{Expr, ExprTrait as _, Query};

use super::corrupt;
use crate::store::apply::insert_event;
use crate::store::db::{Db, DbExt as _, OptionalExtension as _, Tx};
use crate::store::StoreError;

/// The Box and step a run held before a move, which a refusal drops from the row.
pub(super) struct Held {
    /// The Box column.
    box_id: Option<i64>,
    /// The step column.
    step: Option<String>,
}

/// Reads what run `key` holds, before its move.
pub(super) fn held(
    tx: &dyn Db,
    (workflow, repo, due_at): (WorkflowId, &ForgeRepo, Ts),
) -> Result<Held, StoreError> {
    let row: Option<Held> = tx
        .row(
            Query::select()
                .column(WorkflowRuns::Box)
                .column(WorkflowRuns::Step)
                .from(WorkflowRuns::Table)
                .and_where(Expr::col(WorkflowRuns::Workflow).eq(workflow.get()))
                .and_where(Expr::col(WorkflowRuns::Repo).eq(repo.to_string()))
                .and_where(Expr::col(WorkflowRuns::DueAt).eq(due_at.as_millis())),
            |r| {
                Ok(Held {
                    box_id: r.get(0)?,
                    step: r.get(1)?,
                })
            },
        )
        .optional()?;
    Ok(row.unwrap_or(Held {
        box_id: None,
        step: None,
    }))
}

/// Writes the event for run `run`'s move to `to`, if it is one the owner hears of.
///
/// A refusal before any Box was recorded has no Box to carry an event, so it writes none.
pub(super) fn record(
    tx: &Tx<'_>,
    (key, run): ((WorkflowId, &ForgeRepo), i64),
    held: &Held,
    to: &RunState,
    at: Ts,
) -> Result<(), StoreError> {
    let (workflow, repo) = key;
    let (name, prompt) = workflow_of(tx, workflow)?;
    let link = format!("/workflows/{name}/runs/{run}");
    let repo = repo.to_string();
    let (box_id, event) = match to {
        RunState::Waiting {
            box_id, at: step, ..
        } => {
            let question = question_of(tx, run, &prompt, step.as_str())?;
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
/// decline, a skip, and a refusal that never had a Box.
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
                RunOutcome::Delivered { .. } | RunOutcome::Declined { .. } => return None,
            };
            Some((box_id.get(), kind, Some(at.as_str().to_owned()), reason))
        }
        RunEnd::SkippedOverlap | RunEnd::SkippedFork { .. } => None,
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
