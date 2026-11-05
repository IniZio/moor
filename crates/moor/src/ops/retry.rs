//! `retry`: re-run setup on a Box whose setup failed.
//!
//! The Box lock is taken by the provisioning step, which reads the row again under it.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::view::{AgentLaunchView, StartView};
use moor_core::{BoxReq, BoxTarget, ErrorKind};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::provision::{reprovision, Provisioned};
use crate::ops::common::target::{resolve, Rule};
use crate::ops::common::view::{box_view, hide_unsupported_sleep};
use crate::ops::start::launch_after_retry;
use crate::store::BoxRow;

/// Run `retry`.
pub(crate) fn run(cx: &Cx, req: &BoxReq) -> Result<StartView, CommandError> {
    let store = cx.store()?;
    let name = req.box_name.as_str();
    let row = resolve(store, &BoxTarget::Name(req.box_name.clone()), Rule::Open)?
        .ok_or_else(|| CommandError::new(ErrorKind::Failed, format!("no open Box {name}")))?;
    if row.lifecycle != BoxLifecycle::SetupFailed {
        return Err(CommandError::new(
            ErrorKind::Refused,
            format!(
                "not_setup_failed: Box {name} is {}, and only a Box whose setup failed can be retried",
                row.lifecycle.as_str()
            ),
        ));
    }
    store.forget_egress_delivery(row.id)?;
    let Some(provisioned) = reprovision(cx, store, &row, "retry")? else {
        return Err(CommandError::new(
            ErrorKind::Failed,
            format!("Box {name} has no machine that can run setup again"),
        )
        .with_hint(format!("wake it with `moor wake {name}`, then retry")));
    };
    let agent = if provisioned.failed_step().is_some() {
        AgentLaunchView::default()
    } else {
        let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, row.id, "retry", boxlock::WAIT)?;
        launch_after_retry(cx, store, row.id)?
    };
    let row = store
        .box_row(row.id)?
        .ok_or_else(|| CommandError::new(ErrorKind::Internal, "retried Box vanished"))?;
    Ok(view(cx, &row, &provisioned, agent))
}

/// The value `start` gives for an existing Box, with what the retry ran.
fn view(cx: &Cx, row: &BoxRow, p: &Provisioned, agent: AgentLaunchView) -> StartView {
    let mut boxed = box_view(row, cx.config());
    hide_unsupported_sleep(&mut boxed, cx);
    let failed = p.failed_step();
    StartView {
        attached: true,
        queued: row.lifecycle == BoxLifecycle::Pending,
        promoted: Vec::new(),
        work_tree: p.work_tree.clone(),
        setup: p.setup.view(),
        dotfiles: p.dotfiles.view(),
        agent,
        warnings_inline: Vec::new(),
        failed_step: failed.map(str::to_owned),
        handle: failed.and_then(|_| boxed.driver.as_ref().and_then(|b| b.handle.clone())),
        boxed,
    }
}

#[cfg(test)]
mod tests;
