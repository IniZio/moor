//! Parking and restoring a Box's tunnel on behalf of sleep and wake.

use moor_core::{BoxId, ErrorKind};

use crate::cx::Cx;
use crate::error::CommandError;

/// Closes the Box's tunnel and frees its machine's slot while the Box sleeps.
pub(crate) fn park_tunnel(cx: &Cx, box_id: BoxId) -> Result<(), CommandError> {
    cx.proxy_host()
        .park(box_id)
        .map_err(|e| CommandError::new(ErrorKind::Failed, e.to_string()))
}

/// Brings the Box's tunnel back up.
pub(crate) fn restore_tunnel(cx: &Cx, box_id: BoxId) -> Result<(), CommandError> {
    cx.proxy_host()
        .restore(box_id)
        .map_err(|e| CommandError::new(ErrorKind::Failed, e.to_string()))
}
