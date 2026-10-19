//! Builds the response a start returns, from the Task row and the launch outcome.

use std::path::Path;

use moor_core::config::Flags;
use moor_core::domain::task::TaskState;
use moor_core::view::{AgentLaunchView, DotfilesView, SetupView, StartView};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::capacity;
use crate::ops::common::facts::settings;
use crate::ops::common::provision::Provisioned;
use crate::ops::common::view::{hide_unsupported_sleep, task_view};
use crate::store::TaskRow;

/// Puts the launch outcome in the view, naming a failure in a warning too.
pub(super) fn set_agent(v: &mut StartView, agent: AgentLaunchView) {
    if let Some(f) = &agent.reason {
        v.warnings_inline.push(f.text());
    }
    v.agent = agent;
}

/// [`queued_view`] for a row whose provider is not at hand; resolved only when it is queued.
pub(super) fn with_provider(
    cx: &Cx,
    row: &TaskRow,
    attached: bool,
) -> Result<StartView, CommandError> {
    if row.state != TaskState::WaitingForBox {
        return Ok(view(cx, row, attached));
    }
    let at = row.worktree.as_deref().unwrap_or(&row.repo);
    let cfg = settings(cx, Path::new(at), &Flags::default())?;
    Ok(queued_view(cx, row, attached, &cfg.provider.value))
}

/// The response for `row`, with the limit that queued it named in a warning.
pub(super) fn queued_view(cx: &Cx, row: &TaskRow, attached: bool, provider: &str) -> StartView {
    let mut v = view(cx, row, attached);
    if v.queued {
        if let Some(lim) = capacity::limit(cx.config(), provider) {
            v.warnings_inline.push(format!(
                "waiting for a free box: {} allows {} at a time",
                lim.key, lim.max
            ));
        }
    }
    v
}

/// The response for `row`; nothing past the Box runs yet.
pub(super) fn view(cx: &Cx, row: &TaskRow, attached: bool) -> StartView {
    let mut task = task_view(row, cx.config());
    hide_unsupported_sleep(&mut task, cx);
    StartView {
        task,
        attached,
        queued: row.state == TaskState::WaitingForBox,
        promoted: Vec::new(),
        setup: SetupView::default(),
        dotfiles: DotfilesView::default(),
        agent: AgentLaunchView::default(),
        warnings_inline: Vec::new(),
        failed_step: None,
        boxed: None,
    }
}

/// Puts what provisioning ran in the view; a failed setup line also names the step and the Box.
pub(super) fn set_provisioned(v: &mut StartView, p: &Provisioned) {
    v.setup = p.setup.view();
    v.dotfiles = p.dotfiles.view();
    if p.setup.failed() {
        v.failed_step = Some("setup".to_owned());
        v.boxed = v.task.r#box.as_ref().and_then(|b| b.handle.clone());
    }
}
