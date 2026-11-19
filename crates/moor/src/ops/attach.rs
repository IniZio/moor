//! `attach`: the command that relays the agent's terminal, or a login shell, on a Box. The
//! controller socket runs it under a pty and relays it to the caller.

use moor_core::boxscript::quote;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::event::AccessEvent;
use moor_core::domain::hands::HandsState;
use moor_core::seam::driver::Place;
use moor_core::{AttachReq, ErrorKind};

use crate::cx::owners::Cmd;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::provision::recorded_workdir;
use crate::ops::common::target::{self, Rule};

/// The command that relays the agent's terminal on the open Box `req` names, or with
/// `req.shell` a login shell in the Box's work tree.
///
/// # Errors
/// [`CommandError`] when the Box is not open, is asleep, has no machine yet, or its connect
/// command cannot be looked up.
pub(crate) fn argv(cx: &Cx, req: &AttachReq) -> Result<Vec<String>, CommandError> {
    let argv = command(cx, req)?;
    note_opened(cx, req);
    Ok(argv)
}

/// Records that someone opened the terminal of the Box `req` names, which holds it open. A Box
/// that cannot take the record now just goes without it: the terminal opens all the same.
fn note_opened(cx: &Cx, req: &AttachReq) {
    let Ok(store) = cx.store() else { return };
    let Ok(Some(row)) = target::resolve(store, &req.target, Rule::Open) else {
        return;
    };
    let Ok(_turn) = cx.turn(row.id, Cmd::Exec) else {
        return;
    };
    let opened = ExtraEvent::Access(AccessEvent::TerminalOpened { shell: req.shell });
    let _ = store.apply(Decision::new(row.id).event(opened));
}

/// The command [`argv`] gives, before the open is recorded.
fn command(cx: &Cx, req: &AttachReq) -> Result<Vec<String>, CommandError> {
    let store = cx.store()?;
    let row = target::resolve(store, &req.target, Rule::Open)?.ok_or_else(|| {
        CommandError::new(ErrorKind::Failed, format!("no open Box {}", req.target))
    })?;
    let refused = |why: &str| {
        CommandError::new(
            ErrorKind::Refused,
            format!("Box {} {why}", row.name.as_str()),
        )
    };
    let boxed = row
        .hands
        .as_ref()
        .ok_or_else(|| refused("has no machine yet"))?;
    if row.lifecycle == BoxLifecycle::Asleep || boxed.state == HandsState::Asleep {
        return Err(refused("is asleep; wake it first"));
    }
    let handle = boxed
        .handle
        .as_ref()
        .ok_or_else(|| refused("has no machine yet"))?;
    let place = if req.shell { Place::Work } else { Place::Agent };
    let connect = cx
        .driver()
        .connect_argv(handle, place)
        .map_err(|e| CommandError::new(ErrorKind::Failed, e.to_string()))?;
    if req.shell {
        let recorded = boxed.workdir.as_deref().and_then(recorded_workdir);
        let workdir = match recorded {
            Some(w) => w,
            None => cx.driver().workdir(handle)?,
        };
        let mut argv = connect;
        argv.push(format!(
            "cd {} && exec \"${{SHELL:-bash}}\" -l",
            quote(workdir.as_str())
        ));
        return Ok(argv);
    }
    store.require_hc_holder(row.id, row.name.as_str())?;
    cx.engine()
        .relay_argv(&connect)
        .ok_or_else(|| refused("runs an agent with no terminal to attach to"))
}

#[cfg(test)]
mod tests;
