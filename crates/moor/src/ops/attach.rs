//! `attach`: the command that relays the agent's terminal, or a login shell, on a Box. The
//! controller socket runs it under a pty and relays it to the caller.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::boxscript::quote;
use moor_core::domain::boxes::BoxState;
use moor_core::{AttachReq, ErrorKind};

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
        .boxed
        .as_ref()
        .ok_or_else(|| refused("has no machine yet"))?;
    if row.lifecycle == BoxLifecycle::Asleep || boxed.state == BoxState::Asleep {
        return Err(refused("is asleep; wake it first"));
    }
    let handle = boxed
        .handle
        .as_ref()
        .ok_or_else(|| refused("has no machine yet"))?;
    let connect = cx
        .driver()
        .connect_argv(handle)
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
    cx.engine()
        .relay_argv(&connect)
        .ok_or_else(|| refused("runs an agent with no terminal to attach to"))
}

#[cfg(test)]
mod tests;
