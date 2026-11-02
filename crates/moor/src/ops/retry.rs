//! `retry`: re-run setup on a Box whose setup failed.

use moor_core::view::sleep::SleepValue;
use moor_core::{BoxReq, ErrorKind};

use crate::cx::Cx;
use crate::error::CommandError;

/// Run `retry`.
pub(crate) fn run(_cx: &Cx, _req: &BoxReq) -> Result<SleepValue, CommandError> {
    Err(CommandError::new(
        ErrorKind::Failed,
        "retry is not supported yet",
    ))
}
