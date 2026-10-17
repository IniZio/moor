//! `moor close`: gate, save the agent's transcript, stop the agent, tear down and release the Box, then close the Workspace.

use std::path::Path;

use moor_core::decide::close::{
    close_gate, release_outcome, BoxCheck, CloseFacts, ClosePlan, ReleaseResult,
};
use moor_core::decide::transcript::Trigger;
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::seam::driver::{DriverError, Released};
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::{PaneRef, SurfaceError, WorkspaceRef};
use moor_core::view::CloseView;
use moor_core::{BoxHandle, CloseReq, ErrorKind, TaskName, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::boxwork::{self, probe, BoxWork, BOXWORK_DEADLINE};
use crate::ops::common::promote::promote_waiting;
use crate::ops::common::reports;
use crate::ops::common::transcript::{self, Saved};
use crate::ops::common::view::task_view;
use crate::ops::common::wake::wake_box_if_asleep;
use crate::store::{BoxRow, Store, TaskRow};

/// Closes a task: gate, transcript save, agent stop, Box teardown and release, Workspace close.
///
/// The Box lock is held from the gate to the Workspace close and dropped before promotion, which
/// takes the locks of the waiting tasks. The Box is released before the Workspace closes. The
/// worktree goes too, before the Workspace, but only when moor created it and its work is pushed
/// and clean. The agent stops first, so the session stays ended when the release fails and the
/// task stays open. The Task's proxy supervisor stops after the lock is dropped: the wait can take
/// seconds and a closed Task cannot get a new proxy row. Waiting tasks are then promoted.
pub(crate) fn run(cx: &Cx, req: &CloseReq) -> Result<CloseView, CommandError> {
    let store = cx.store()?;
    let first = find(store, &req.task)?;
    let lock = BoxLock::acquire(
        cx.clock(),
        cx.state_dir()?,
        first.id,
        "close",
        boxlock::WAIT,
    )?;
    let mut row = store.task(first.id)?.unwrap_or(first);
    let box_check = box_check(cx, store, &mut row, req.force)?;
    let probed_clean = matches!(box_check, BoxCheck::Clean);
    let plan = close_gate(&facts(cx, &row, box_check)?, req.force)?;
    let saved = save_transcript(cx, store, &row)?;
    let digest = saved.as_ref().map(|t| t.digest.as_str());
    let agent_stopped = stop_agent(cx, store, &row, digest)?;
    if probed_clean {
        recheck_and_collect(cx, store, &row)?;
    } else {
        reports::collect(cx, store, &row);
    }
    let box_deleted = if plan.release_box {
        teardown::teardown(cx, &row, req.trust_moor_yaml);
        release(cx, store, &row)?
    } else {
        store.apply(plan.decision_close.clone())?;
        false
    };
    let synced = worktree::sync_branch(cx, &row);
    let worktree_removed = worktree::remove_worktree(cx, &row, synced);
    let workspace_closed = close_workspace(cx, &row, &plan);
    drop(lock);
    if let Err(e) = cx.proxy_host().stop(row.id) {
        cx.warn(Warning {
            code: "proxy_stop_failed".to_owned(),
            message: format!("{e}; the task is closed anyway"),
        });
    }
    let provider = row.boxed.as_ref().map(|b| b.provider.clone());
    let closed = store.task(row.id)?.unwrap_or(row);
    let promoted = provider.map_or_else(Vec::new, |p| {
        promote_waiting(cx, store, &p, "close")
            .iter()
            .map(|n| n.as_str().to_owned())
            .collect()
    });
    Ok(CloseView {
        task: task_view(&closed, cx.config()),
        box_deleted,
        workspace_closed,
        agent_stopped,
        transcript: saved.map(|t| t.path),
        worktree_kept: !worktree_removed,
        promoted,
    })
}

/// Saves the live agent's transcript while the Box is still Ready; a failure only records an event.
fn save_transcript(cx: &Cx, store: &Store, row: &TaskRow) -> Result<Option<Saved>, CommandError> {
    let Some(session) = store.live_session(row.id)? else {
        return Ok(None);
    };
    Ok(transcript::save(cx, store, row, &session, Trigger::Close))
}

/// Closes the live agent pane and ends its session, with the digest of its saved transcript;
/// a pane already gone is fine.
///
/// Returns whether a session was ended.
fn stop_agent(
    cx: &Cx,
    store: &Store,
    row: &TaskRow,
    digest: Option<&str>,
) -> Result<bool, CommandError> {
    let Some(session) = store.live_session(row.id)? else {
        return Ok(false);
    };
    if let Some(pane) = session.pane.as_deref() {
        match cx.surface().close_pane(&PaneRef::new(pane)) {
            Ok(()) | Err(SurfaceError::NotFound) => {}
            Err(e) => {
                return Err(CommandError::new(
                    ErrorKind::Failed,
                    format!("could not close the agent pane: {e}; the task stays open"),
                )
                .with_hint("try `moor close` again, or close the pane by hand"));
            }
        }
    }
    store.end_session(session.id, None, digest)?;
    store.apply(Decision::new(row.id).event(ExtraEvent::SessionsStopped {
        sessions: vec![session.session_ref],
    }))?;
    Ok(true)
}

/// The open task by name, else the newest closed one (the gate refuses it).
fn find(store: &Store, name: &TaskName) -> Result<TaskRow, CommandError> {
    if let Some(row) = store.open_task_by_name(name)? {
        return Ok(row);
    }
    store.latest_task_by_name(name)?.ok_or_else(|| {
        CommandError::new(
            ErrorKind::Failed,
            format!("no task named {}", name.as_str()),
        )
    })
}

/// The Driver handle to release: the acquired one, else one a failed acquire left.
const fn held(b: &BoxRow) -> Option<&BoxHandle> {
    match (&b.handle, &b.attempted_handle) {
        (Some(h), _) | (None, Some(h)) => Some(h),
        (None, None) => None,
    }
}

/// Looks for work that exists only in the Box, waking a sleeping Box to read it.
///
/// `row` is reloaded when the wake changed it, so teardown sees a Ready Box. A Box that
/// cannot be woken or read gives [`BoxCheck::Unknown`], which the gate refuses.
fn box_check(
    cx: &Cx,
    store: &Store,
    row: &mut TaskRow,
    force: bool,
) -> Result<BoxCheck, CommandError> {
    // --force discards Box work, so the Box is neither woken nor probed.
    if force {
        return Ok(BoxCheck::Skipped);
    }
    let live = |r: &TaskRow| {
        r.boxed.as_ref().is_some_and(|b| {
            b.handle.is_some() && !matches!(b.state, BoxState::Gone | BoxState::Deleted)
        })
    };
    if !live(row) {
        return Ok(BoxCheck::Skipped);
    }
    // The tunnel stays parked: close needs the Box awake, not reachable through the Proxy.
    match wake_box_if_asleep(cx, store, row) {
        Ok(true) => {
            if let Some(fresh) = store.task(row.id)? {
                *row = fresh;
            }
        }
        Ok(false) => {}
        // The driver has forgotten the Box: nothing to save, and release treats it as deleted.
        Err(e) if e.error.downcast_ref::<DriverError>() == Some(&DriverError::NotFound) => {
            return Ok(BoxCheck::Skipped)
        }
        Err(e) => {
            return Ok(BoxCheck::Unknown(format!(
                "could not wake the Box: {}",
                e.error
            )))
        }
    }
    // A Box that never got ready, or whose release already failed, has no reachable work tree.
    let Some(b) = row.boxed.as_ref().filter(|b| b.state == BoxState::Ready) else {
        return Ok(BoxCheck::Skipped);
    };
    Ok(match probe(cx, row, b) {
        BoxWork::Clean => BoxCheck::Clean,
        BoxWork::Unsaved { ahead, dirty } => BoxCheck::Unsaved { ahead, dirty },
        BoxWork::Gone => BoxCheck::Skipped,
        BoxWork::Unknown(why) => BoxCheck::Unknown(why),
        BoxWork::NoCheckout => BoxCheck::Unknown("the Box has no git checkout".into()),
    })
}

/// Probes the Box again once the agent is stopped, then collects its Reports; a live agent can
/// write between the first probe and the stop. The agent stays stopped when this refuses, and
/// no Report is stored then. The probe and the Report index run as one batch; when it stops
/// early, Reports are collected on their own.
fn recheck_and_collect(cx: &Cx, store: &Store, row: &TaskRow) -> Result<(), CommandError> {
    let Some(b) = row.boxed.as_ref().filter(|b| b.state == BoxState::Ready) else {
        reports::collect(cx, store, row);
        return Ok(());
    };
    let index = reports::index_plan(cx, row);
    let mut done = Vec::new().into_iter();
    let work = match (boxwork::plan(cx, row, b), b.handle.as_ref(), &index) {
        (Err(work), _, _) => work,
        (Ok(plan), Some(handle), Some(index)) => {
            let steps = [plan.steps.as_slice(), index.steps.as_slice()].concat();
            let host = Path::new(row.worktree.as_deref().unwrap_or(&row.repo));
            // The probe and the index script each get their own allowance.
            let deadline = Deadline(BOXWORK_DEADLINE + reports::INDEX_DEADLINE);
            done = cx
                .driver()
                .batch(handle, &[], &steps, host, deadline)
                .into_iter();
            boxwork::read(&plan, &mut done)
        }
        (Ok(_), _, _) => boxwork::probe(cx, row, b),
    };
    recheck_verdict(cx, row, work)?;
    index.as_ref().map_or_else(
        || reports::collect(cx, store, row),
        |i| reports::collect_from(cx, store, row, i, &mut done),
    );
    Ok(())
}

/// Refuses when the second probe found unsaved work or could not tell.
fn recheck_verdict(cx: &Cx, row: &TaskRow, work: BoxWork) -> Result<(), CommandError> {
    let check = match work {
        BoxWork::Clean | BoxWork::Gone => return Ok(()),
        BoxWork::Unsaved { ahead, dirty } => BoxCheck::Unsaved { ahead, dirty },
        BoxWork::Unknown(why) => BoxCheck::Unknown(why),
        BoxWork::NoCheckout => BoxCheck::Unknown("the Box has no git checkout".into()),
    };
    match close_gate(&facts(cx, row, check)?, false) {
        Ok(_) => Ok(()),
        Err(refusal) => Err(CommandError::from(refusal).with_hint(format!(
            "the agent was stopped; `moor start {} --resume` continues it",
            row.name.as_str()
        ))),
    }
}

/// Reads the unsaved-work facts the gate decides on.
fn facts(cx: &Cx, row: &TaskRow, box_check: BoxCheck) -> Result<CloseFacts, CommandError> {
    let (mut unpushed, mut dirty) = (0, false);
    if let Some(wt) = row.worktree.as_deref().map(Path::new) {
        let failed = |e| CommandError::new(ErrorKind::Failed, e);
        unpushed = cx.git().unpushed(wt, row.base.as_deref()).map_err(failed)?;
        dirty = cx.git().dirty(wt).map_err(failed)?;
    }
    Ok(CloseFacts {
        task: row.id,
        name: row.name.as_str().to_owned(),
        branch: row.branch.clone(),
        state: row.state,
        box_state: row.boxed.as_ref().map(|b| b.state),
        has_handle: row.boxed.as_ref().is_some_and(|b| held(b).is_some()),
        unpushed,
        dirty,
        box_check,
    })
}

/// Asks the Driver to release the Box and records the verdict.
///
/// Returns whether the Box is now deleted.
fn release(cx: &Cx, store: &Store, row: &TaskRow) -> Result<bool, CommandError> {
    let Some(b) = row.boxed.as_ref() else {
        return Ok(false);
    };
    let Some(handle) = held(b) else {
        return Ok(false);
    };
    let (result, cause) = match cx.driver().release(handle) {
        Ok(Released::Deleted | Released::AlreadyGone) => (ReleaseResult::Deleted, None),
        Err(e) => (ReleaseResult::Failed, Some(e)),
    };
    let verdict = release_outcome(row.id, b.state, result);
    if let Some(d) = verdict.decision {
        store.apply(d)?;
    }
    if let Some(w) = verdict.warning {
        cx.warn(w);
    }
    if !verdict.closes {
        let why = cause.map_or_else(
            || "the Box could not be released".to_owned(),
            |e| e.to_string(),
        );
        return Err(
            CommandError::new(ErrorKind::Failed, format!("{why}; the task stays open"))
                .with_hint("try `moor close` again, or release the Box by hand"),
        );
    }
    Ok(result == ReleaseResult::Deleted)
}

/// Closes the Workspace; a missing one is success, other failures warn.
fn close_workspace(cx: &Cx, row: &TaskRow, plan: &ClosePlan) -> bool {
    let Some(id) = row.workspace_id.as_deref().filter(|_| plan.close_workspace) else {
        return false;
    };
    match cx.surface().close_workspace(&WorkspaceRef::new(id)) {
        Ok(()) | Err(SurfaceError::NotFound) => true,
        Err(_) => {
            cx.warn(Warning {
                code: "workspace_close_failed".to_owned(),
                message: "the task is closed but its Workspace could not be closed; \
                          close it by hand"
                    .to_owned(),
            });
            false
        }
    }
}

pub(crate) mod teardown;
mod worktree;

#[cfg(test)]
mod tests;
