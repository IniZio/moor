//! `close-failed`: close every Box whose setup failed.

use moor_core::request::CloseFailedReq;
use moor_core::view::close_failed::CloseFailedView;
use moor_core::ErrorKind;

use crate::cx::Cx;
use crate::error::CommandError;

/// Run `close-failed`.
pub(crate) fn run(_cx: &Cx, _req: &CloseFailedReq) -> Result<CloseFailedView, CommandError> {
    Err(CommandError::new(
        ErrorKind::Failed,
        "close-failed is not supported yet",
    ))
}
