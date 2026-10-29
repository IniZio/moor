//! `moor close`: gate, save the agent's transcript, stop the agent, tear down and release the Box, then close the Workspace.

use moor_core::config::Flags;
use moor_core::decide::close::{
    close_gate, needs_box_check, release_outcome, BoxCheck, CloseFacts, ClosePlan, Refusal,
    ReleaseResult,
};
use moor_core::decide::reconcile::{reconcile, BoxFact};
use moor_core::decide::transcript::Trigger;
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::seam::driver::{DriverError, Released};
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::{SurfaceError, WorkspaceRef};
use moor_core::view::CloseView;
use moor_core::{BoxHandle, CloseReq, ErrorKind, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::agent_stop;
use crate::ops::common::artifacts;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::boxwork::{self, probe, BoxWork, BOXWORK_DEADLINE};
use crate::ops::common::cache;
use crate::ops::common::facts::settings_of;
use crate::ops::common::promote::promote_waiting;
use crate::ops::common::provision::planned_root;
use crate::ops::common::reports;
use crate::ops::common::target::{self, Rule};
use crate::ops::common::transcript::{self, Saved};
use crate::ops::common::view::box_view;
use crate::ops::common::wake::wake_box_if_asleep;
use crate::store::{BoxRow, DriverRow, Store};

/// Closes a Box: gate, transcript save, agent stop, machine teardown and release, Workspace close.
///
/// The Box lock is held from the gate to the Workspace close and dropped before promotion, which
/// takes the locks of the waiting Boxes. The machine is released before the Workspace closes.
/// The agent stops first, so the session stays ended when the release fails and the
/// Box stays open. The Box's proxy supervisor stops after the lock is dropped: the wait can take
/// seconds and a closed Box cannot get a new proxy row. Waiting Boxes are then promoted.
pub(crate) fn run(cx: &Cx, req: &CloseReq) -> Result<CloseView, CommandError> {
    let store = cx.store()?;
    let first = target::resolve(store, &req.target, Rule::OpenElseLatest)?
        .ok_or_else(|| CommandError::new(ErrorKind::Failed, format!("no Box {}", req.target)))?;
    let lock = BoxLock::acquire(
        cx.clock(),
        cx.state_dir()?,
        first.id,
        "close",
        boxlock::WAIT,
    )?;
    let mut row = store.box_row(first.id)?.unwrap_or(first);
    with_planned(store, &mut row)?;
    let box_check = box_check(cx, store, &mut row, req.force)?;
    let probed_clean = matches!(box_check, BoxCheck::Clean);
    let plan = close_gate(&facts(&row, box_check), req.force).map_err(|r| refused(&row, r))?;
    let saved = save_transcript(cx, store, &row)?;
    let digest = saved.as_ref().map(|t| t.digest.as_str());
    let agent_stopped = agent_stop::stop_agent(cx, store, &row, digest)?;
    let box_deleted = wind_down(cx, store, &row, &plan, req.force, probed_clean)?;
    let workspace_closed = close_workspace(cx, &row, &plan);
    drop(lock);
    if let Err(e) = cx.proxy_host().stop(row.id) {
        cx.warn(Warning {
            code: "proxy_stop_failed".to_owned(),
            message: format!("{e}; the Box is closed anyway"),
        });
    }
    let provider = row.boxed.as_ref().map(|b| b.provider.clone());
    let closed = store.box_row(row.id)?.unwrap_or(row);
    let mut promoted = Vec::new();
    for name in provider.map_or_else(Vec::new, |p| promote_waiting(cx, store, &p, "close")) {
        if let Some(row) = store.open_box_by_name(&name)? {
            if cx.viewer().may_act_on(row.user) {
                promoted.push(name.as_str().to_owned());
            }
        }
    }
    Ok(CloseView {
        boxed: box_view(&closed, cx.config()),
        box_deleted,
        workspace_closed,
        agent_stopped,
        transcript: saved.map(|t| t.path),
        promoted,
    })
}

/// Saves the live agent's transcript while the Box is still Ready; a failure only records an event.
fn save_transcript(cx: &Cx, store: &Store, row: &BoxRow) -> Result<Option<Saved>, CommandError> {
    let Some(session) = store.live_session(row.id)? else {
        return Ok(None);
    };
    Ok(transcript::save(cx, store, row, &session, Trigger::Close))
}

/// The Driver handle to release: the acquired one, else one a failed acquire left.
const fn held(b: &DriverRow) -> Option<&BoxHandle> {
    match (&b.handle, &b.attempted_handle) {
        (Some(h), _) | (None, Some(h)) => Some(h),
        (None, None) => None,
    }
}

/// Gives a Box that recorded no handle the one its acquire planned, so the close releases a
/// machine an unfinished acquire may have created.
fn with_planned(store: &Store, row: &mut BoxRow) -> Result<(), CommandError> {
    let id = row.id;
    if let Some(b) = row.boxed.as_mut().filter(|b| held(b).is_none()) {
        b.attempted_handle = planned_root(store, id)?;
    }
    Ok(())
}

/// Looks for work that exists only in the Box, waking a sleeping Box to read it.
///
/// `row` is reloaded when the wake changed it, so teardown sees a Ready Box. A Box that
/// cannot be woken or read gives [`BoxCheck::Unknown`], which the gate refuses.
fn box_check(
    cx: &Cx,
    store: &Store,
    row: &mut BoxRow,
    force: bool,
) -> Result<BoxCheck, CommandError> {
    // --force discards Box work, so the Box is neither woken nor probed.
    if force || !needs_box_check(row.lifecycle, row.session) {
        return Ok(BoxCheck::Skipped);
    }
    let live = |r: &BoxRow| {
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
            if let Some(fresh) = store.box_row(row.id)? {
                *row = fresh;
            }
        }
        Ok(false) => {}
        // The driver has forgotten the Box: nothing to save, and release treats it as deleted.
        Err(e) if e.error.downcast_ref::<DriverError>() == Some(&DriverError::NotFound) => {
            return Ok(BoxCheck::Skipped);
        }
        Err(e) => {
            return Ok(BoxCheck::Unknown(format!(
                "could not wake the Box: {}",
                e.error
            )));
        }
    }
    // A Box that never got ready, or whose release already failed, has no reachable work tree.
    let Some(b) = row.boxed.as_ref().filter(|b| b.state == BoxState::Ready) else {
        return Ok(BoxCheck::Skipped);
    };
    Ok(match probe(cx, row, b) {
        BoxWork::Clean | BoxWork::NoCheckout => BoxCheck::Clean,
        BoxWork::Unsaved { ahead, dirty } => BoxCheck::Unsaved { ahead, dirty },
        BoxWork::Gone => BoxCheck::Skipped,
        BoxWork::Unknown(why) => BoxCheck::Unknown(why),
    })
}

/// Probes the Box again once the agent is stopped, then collects its Reports; a live agent can
/// write between the first probe and the stop. The agent stays stopped when this refuses, and
/// no Report is stored then. The probe and the Report index run as one batch; when it stops
/// early, Reports are collected on their own.
fn recheck_and_collect(cx: &Cx, store: &Store, row: &BoxRow) -> Result<(), CommandError> {
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
            // The probe and the index script each get their own allowance.
            let deadline = Deadline(BOXWORK_DEADLINE + reports::INDEX_DEADLINE);
            done = cx.driver().batch(handle, &[], &steps, deadline).into_iter();
            boxwork::read(&plan, &mut done)
        }
        (Ok(_), _, _) => boxwork::probe(cx, row, b),
    };
    recheck_verdict(row, work)?;
    index.as_ref().map_or_else(
        || reports::collect(cx, store, row),
        |i| reports::collect_from(cx, store, row, i, &mut done),
    );
    collect_artifacts(cx, store, row);
    Ok(())
}

/// Collects the Box's artifacts when it is Ready; a Box that cannot run an exec has none to collect.
fn collect_artifacts(cx: &Cx, store: &Store, row: &BoxRow) {
    if let Some(b) = row.boxed.as_ref() {
        if b.state == BoxState::Ready && b.handle.is_some() {
            let cursor = store.newest_turn_event(row.id);
            artifacts::collect(cx, store, row);
            match cursor {
                Ok(Some(through)) => {
                    if let Err(e) = store.set_artifacts_through(row.id, through) {
                        cx.warn(cursor_warning(&e.to_string()));
                    }
                }
                Ok(None) => {}
                Err(e) => cx.warn(cursor_warning(&e.to_string())),
            }
        }
    }
}

/// Warns that the artifact cursor could not be read or moved.
fn cursor_warning(reason: &str) -> Warning {
    Warning {
        code: "artifacts_collect_failed".to_owned(),
        message: format!("artifacts were not fully saved: {reason}"),
    }
}

/// Refuses when the second probe found unsaved work or could not tell.
fn recheck_verdict(row: &BoxRow, work: BoxWork) -> Result<(), CommandError> {
    let check = match work {
        BoxWork::Clean | BoxWork::NoCheckout | BoxWork::Gone => return Ok(()),
        BoxWork::Unsaved { ahead, dirty } => BoxCheck::Unsaved { ahead, dirty },
        BoxWork::Unknown(why) => BoxCheck::Unknown(why),
    };
    match close_gate(&facts(row, check), false) {
        Ok(_) => Ok(()),
        Err(refusal) => Err(refused(row, refusal).with_hint(format!(
            "the agent was stopped; `moor start {} --resume` continues it",
            row.name.as_str()
        ))),
    }
}

/// The gate's refusal of `row`; the CLI way past an unchecked Box is a hint, so fronts never show it.
fn refused(row: &BoxRow, r: Refusal) -> CommandError {
    let unchecked = matches!(r, Refusal::BoxUnchecked { .. });
    let e = CommandError::from(r);
    if unchecked {
        let name = row.name.as_str();
        return e.with_hint(format!(
            "`moor close {name} --force` closes it anyway and discards any work in the Box"
        ));
    }
    e
}

/// The facts the gate decides on; only the Box's own probe speaks for unsaved work.
fn facts(row: &BoxRow, box_check: BoxCheck) -> CloseFacts {
    CloseFacts {
        box_id: row.id,
        name: row.name.as_str().to_owned(),
        branch: row.branch.clone(),
        lifecycle: row.lifecycle,
        session: row.session,
        box_state: row.boxed.as_ref().map(|b| b.state),
        has_handle: row.boxed.as_ref().is_some_and(|b| held(b).is_some()),
        box_check,
    }
}

/// Seals the cache volumes of a ready Box after its teardown, from the `.moor.yaml` stored with it.
fn seal_caches(cx: &Cx, store: &Store, row: &BoxRow) {
    let ready = row.boxed.as_ref().filter(|b| b.state == BoxState::Ready);
    let Some((b, handle)) = ready.and_then(|b| Some((b, held(b)?))) else {
        return;
    };
    if let Ok(cfg) = settings_of(cx, row, &Flags::default()) {
        cache::seal(cx, store, row, &cfg, handle, &b.provider);
    }
}

/// Collects from the Box, tears it down and releases it; returns whether the Box is now deleted.
///
/// Under `force`, a machine the provider no longer lists is marked Gone with one warning,
/// since collection, teardown and release all need the machine.
fn wind_down(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    plan: &ClosePlan,
    force: bool,
    probed_clean: bool,
) -> Result<bool, CommandError> {
    if force && plan.release_box && machine_vanished(cx, row) {
        cx.warn(Warning {
            code: "box_already_gone".to_owned(),
            message: "the Box's machine was already gone on the provider; teardown and \
                      collecting Reports and artifacts were skipped"
                .to_owned(),
        });
        let gone = plan
            .decision_close
            .clone()
            .box_input(BoxInput::DriverMissing);
        store.apply(gone)?;
        return Ok(true);
    }
    if probed_clean {
        recheck_and_collect(cx, store, row)?;
    } else {
        reports::collect(cx, store, row);
        collect_artifacts(cx, store, row);
    }
    if !plan.release_box {
        store.apply(plan.decision_close.clone())?;
        return Ok(false);
    }
    teardown::teardown(cx, row, force)?;
    seal_caches(cx, store, row);
    release(cx, store, row)
}

/// Whether the provider's inventory no longer lists the Box's machine, as reconcile reads it.
///
/// A listing that fails or is unsupported cannot tell, so the close goes on as usual.
fn machine_vanished(cx: &Cx, row: &BoxRow) -> bool {
    let Some(b) = row.boxed.as_ref() else {
        return false;
    };
    let fact = BoxFact {
        box_id: row.id,
        state: b.state,
        handle: b.handle.clone(),
    };
    cx.driver()
        .list(&b.provider)
        .is_ok_and(|listing| !reconcile(&[fact], &listing).is_empty())
}

/// Asks the Driver to release the Box and records the verdict.
///
/// Returns whether the Box is now deleted.
fn release(cx: &Cx, store: &Store, row: &BoxRow) -> Result<bool, CommandError> {
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
        let by_hand = cx.driver().manual_release(handle).map_or_else(
            || "release the Box by hand".to_owned(),
            |how| format!("release the Box by hand: {how}"),
        );
        return Err(
            CommandError::new(ErrorKind::Failed, format!("{why}; the Box stays open"))
                .with_hint(format!("try `moor close` again, or {by_hand}")),
        );
    }
    Ok(result == ReleaseResult::Deleted)
}

/// Closes the Workspace; a missing one is success, other failures warn.
fn close_workspace(cx: &Cx, row: &BoxRow, plan: &ClosePlan) -> bool {
    let Some(id) = row.workspace_id.as_deref().filter(|_| plan.close_workspace) else {
        return false;
    };
    match cx.surface().close_workspace(&WorkspaceRef::new(id)) {
        Ok(()) | Err(SurfaceError::NotFound) => true,
        Err(_) => {
            cx.warn(Warning {
                code: "workspace_close_failed".to_owned(),
                message: "the Box is closed but its Workspace could not be closed; \
                          close it by hand"
                    .to_owned(),
            });
            false
        }
    }
}

mod teardown;

#[cfg(test)]
mod tests;
