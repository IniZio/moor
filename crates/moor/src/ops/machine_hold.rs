//! `machine-hold`: a Box claims one of its user's machines, or lets go of it. The repo's
//! `.moor.yaml` `machine:` section gives the labels and ports a claim leaves out, and its
//! `machine.sync` what the machine's syncs leave alone.

use moor_core::config::sync_excludes;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::{ErrorKind, HoldValue, MachineHoldReq};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::facts::repo_config;

/// Run `machine-hold` for the open Box `req.id`, among the machines of the user it acts for.
///
/// # Errors
/// No open Box with that id, a Box that acts for no user, a stored `.moor.yaml` that does not
/// parse, or a store failure.
pub(crate) fn run(cx: &Cx, req: &MachineHoldReq) -> Result<HoldValue, CommandError> {
    let store = cx.store()?;
    let row = store
        .box_row(req.id)?
        .filter(|row| !matches!(row.lifecycle, BoxLifecycle::Closed | BoxLifecycle::Rejected))
        .ok_or_else(|| {
            CommandError::new(
                ErrorKind::Failed,
                format!("no open Box with id {}", req.id.get()),
            )
        })?;
    let user = row.user.ok_or_else(|| {
        CommandError::new(
            ErrorKind::Refused,
            "the Box acts for no user, so it has no machines",
        )
    })?;
    let repo = repo_config(&row)?;
    let want = repo.as_ref().and_then(|r| r.machine.as_ref());
    let exclude = want.map_or_else(Vec::new, |w| {
        sync_excludes(&w.sync, &cx.config().machines.sync)
    });
    Ok(store.hold_machine(req.id, user, &req.op, want, &exclude)?)
}

#[cfg(test)]
mod tests;
