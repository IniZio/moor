//! Cancelling a pull request's open run, because a newer head was pushed or the pull request was
//! closed.
//!
//! The run ends `Cancelled` first, in one transaction compare-and-set on its state, so a cancel
//! that lost to a finish, a report or another cancel changes nothing. Its agent is stopped after,
//! and its Box stays for follow-ups as any ended run's Box does. An asleep Box runs no agent, so
//! it is left asleep, never woken to be stopped. A Box whose agent never launched is closed.

use moor_core::name::ForgeRepo;
use moor_core::request::EndSessionReq;
use moor_core::workflow::{CancelCause, Workflow, WorkflowRun};

use super::settle::close_open_box;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::end_session;
use crate::store::Store;

/// Cancels the open run of `w` for pull request `number` on `repo` with `cause`, and stops its
/// agent. The run as it was before the cancel, none when nothing was open to cancel, and what
/// to log.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the closed event that calls it comes with the receiver"
    )
)]
pub(crate) fn cancel_pull(
    cx: &Cx,
    store: &Store,
    w: &Workflow,
    (repo, number): (&ForgeRepo, u64),
    cause: &CancelCause,
) -> Result<(Option<WorkflowRun>, Vec<String>), CommandError> {
    let now = cx.clock().now();
    let Some(run) = store.cancel_pull_run(w.id, (repo, number), cause, now)? else {
        return Ok((None, Vec::new()));
    };
    let out = stop_agent(cx, store, &run).into_iter().collect();
    Ok((Some(run), out))
}

/// Stops the agent of cancelled run `run` when its Box is awake; why it could not, if it could
/// not. A Box whose agent never launched, such as one still waiting for capacity, is closed
/// instead, so it never launches on the stale head later.
pub(super) fn stop_agent(cx: &Cx, store: &Store, run: &WorkflowRun) -> Option<String> {
    let row = match run.state.box_id().map(|id| store.box_row(id)) {
        Some(Ok(Some(row))) => row,
        Some(Err(e)) => {
            return Some(format!(
                "could not read the Box of cancelled run {}: {e}",
                run.id.get()
            ))
        }
        Some(Ok(None)) | None => return None,
    };
    if !row.lifecycle.is_live() {
        return None;
    }
    match store.first_launch_at(row.id) {
        Ok(None) => {
            return close_open_box(cx, store, &row.name).err().map(|why| {
                format!(
                    "could not close the unstarted Box {}: {why}",
                    row.name.as_str()
                )
            });
        }
        Err(e) => {
            return Some(format!(
                "could not read the agent of {}: {e}",
                row.name.as_str()
            ))
        }
        Ok(Some(_)) => {}
    }
    if row.asleep() {
        return None;
    }
    let req = EndSessionReq {
        box_name: row.name.as_str().to_owned(),
    };
    cx.for_box(row.user, || end_session::run(cx, &req))
        .err()
        .map(|e| {
            format!(
                "could not stop the agent of {}: {}",
                row.name.as_str(),
                e.error
            )
        })
}
