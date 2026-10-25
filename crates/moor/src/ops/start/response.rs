//! Builds the response a start returns, from the Box row and the launch outcome.

use std::collections::BTreeSet;

use moor_core::config::Flags;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::user::{SecretName, UserId};
use moor_core::view::{AgentLaunchView, McpLeftOut, DotfilesView, SetupView, StartView};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::capacity;
use crate::ops::common::facts::{provider_of, settings_of};
use crate::ops::common::provision::Provisioned;
use crate::ops::common::view::{box_view, hide_unsupported_sleep};
use crate::store::BoxRow;

/// Puts the launch outcome in the view, naming a failure in a warning too.
pub(super) fn set_agent(v: &mut StartView, agent: AgentLaunchView) {
    if let Some(f) = &agent.reason {
        v.warnings_inline.push(f.text());
    }
    v.agent = agent;
}

/// Names in the view the MCP servers `user` lacks a secret for, which the Box starts without.
pub(super) fn set_mcp_left_out(
    cx: &Cx,
    v: &mut StartView,
    user: UserId,
) -> Result<(), CommandError> {
    let have: BTreeSet<SecretName> = cx.store()?.user_secret_names(user)?.into_iter().collect();
    v.mcp_left_out = cx
        .config()
        .mcp_left_out(&have)
        .into_iter()
        .map(|(server, secrets)| McpLeftOut { server, secrets })
        .collect();
    Ok(())
}

/// [`queued_view`] for a row whose provider is not at hand; resolved only when it is queued.
pub(super) fn with_provider(
    cx: &Cx,
    row: &BoxRow,
    attached: bool,
) -> Result<StartView, CommandError> {
    if row.lifecycle != BoxLifecycle::WaitingForBox {
        return Ok(view(cx, row, attached));
    }
    let cfg = settings_of(cx, row, &Flags::default())?;
    Ok(queued_view(cx, row, attached, provider_of(&cfg).ok()))
}

/// The response for `row`, with the limit that queued it named in a warning.
pub(super) fn queued_view(
    cx: &Cx,
    row: &BoxRow,
    attached: bool,
    provider: Option<&str>,
) -> StartView {
    let mut v = view(cx, row, attached);
    if v.queued {
        if let Some(lim) = provider.and_then(|p| capacity::limit(cx.config(), p)) {
            v.warnings_inline.push(format!(
                "waiting for a free box: {} allows {} at a time",
                lim.key, lim.max
            ));
        }
    }
    v
}

/// The response for `row`; nothing past the Box runs yet.
pub(super) fn view(cx: &Cx, row: &BoxRow, attached: bool) -> StartView {
    let mut view = box_view(row, cx.config());
    hide_unsupported_sleep(&mut view, cx);
    StartView {
        boxed: view,
        attached,
        queued: row.lifecycle == BoxLifecycle::WaitingForBox,
        promoted: Vec::new(),
        work_tree: None,
        setup: SetupView::default(),
        dotfiles: DotfilesView::default(),
        agent: AgentLaunchView::default(),
        warnings_inline: Vec::new(),
        mcp_left_out: Vec::new(),
        failed_step: None,
        handle: None,
    }
}

/// Puts what provisioning ran in the view; a failed setup line also names the step and the Box.
pub(super) fn set_provisioned(v: &mut StartView, p: &Provisioned) {
    v.setup = p.setup.view();
    v.dotfiles = p.dotfiles.view();
    v.work_tree.clone_from(&p.work_tree);
    if p.setup.failed() {
        v.failed_step = Some("setup".to_owned());
        v.handle = v.boxed.driver.as_ref().and_then(|b| b.handle.clone());
    }
}
