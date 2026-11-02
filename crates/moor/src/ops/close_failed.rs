//! `close-failed`: close every Box whose setup failed.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::event::close_reason::CLOSE_REASON_CLOSE_FAILED;
use moor_core::request::CloseFailedReq;
use moor_core::view::close_failed::{CloseFailedError, CloseFailedView};
use moor_core::CloseReq;

use super::close;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::BoxRow;

/// Run `close-failed`.
pub(crate) fn run(cx: &Cx, req: &CloseFailedReq) -> Result<CloseFailedView, CommandError> {
    let rows: Vec<BoxRow> = cx
        .store()?
        .open_boxes()?
        .into_iter()
        .filter(|r| r.lifecycle == BoxLifecycle::SetupFailed && cx.viewer().may_act_on(r.user))
        .collect();
    Ok(close_setup_failed(
        cx,
        &rows,
        CLOSE_REASON_CLOSE_FAILED,
        req.force,
    ))
}

/// Closes each of `rows` for `reason`, one after another.
///
/// A Box that will not close goes into `failed` and the rest still close; nothing is retried.
pub(crate) fn close_setup_failed(
    cx: &Cx,
    rows: &[BoxRow],
    reason: &str,
    force: bool,
) -> CloseFailedView {
    let mut view = CloseFailedView {
        closed: Vec::new(),
        failed: Vec::new(),
    };
    for row in rows {
        let req = CloseReq {
            target: row.id.into(),
            force,
        };
        match close::run_with_reason(cx, &req, Some(reason.to_owned())) {
            Ok(_) => view.closed.push(row.name.clone()),
            Err(e) => view.failed.push(CloseFailedError {
                r#box: row.name.clone(),
                error: e.error.to_string(),
            }),
        }
    }
    view
}

#[cfg(test)]
mod tests;
