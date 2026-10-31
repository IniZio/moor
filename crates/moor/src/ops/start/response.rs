//! Builds the response a start returns, from the Box row and the launch outcome.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::view::{AgentLaunchView, DotfilesView, SetupView, StartView};

use crate::cx::Cx;
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

/// The response for `row`, with why it waits named in a warning when it is queued.
pub(super) fn queued_view(cx: &Cx, row: &BoxRow, attached: bool) -> StartView {
    let mut v = view(cx, row, attached);
    if v.queued {
        v.warnings_inline.extend(row.state_reason.clone());
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
        queued: row.lifecycle == BoxLifecycle::Pending,
        promoted: Vec::new(),
        work_tree: None,
        setup: SetupView::default(),
        dotfiles: DotfilesView::default(),
        agent: AgentLaunchView::default(),
        warnings_inline: crate::ops::users::mcp::for_box(cx, row.user)
            .map(|m| m.notes)
            .unwrap_or_default(),
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
