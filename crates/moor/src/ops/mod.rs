//! Command operations: one op per request.

use moor_core::{ErrorKind, Request};

use crate::cx::Cx;
use crate::error::CommandError;

mod version;

#[cfg(test)]
mod tests;

/// Route a request to its op.
#[expect(unused_variables, reason = "cx is used by ops from S3")]
pub(crate) fn dispatch(cx: &Cx, req: Request) -> Result<serde_json::Value, CommandError> {
    match req {
        Request::Version => serde_json::to_value(version::exec())
            .map_err(|e| CommandError::new(ErrorKind::Internal, e)),
        other => Err(CommandError::not_implemented(other.cmd())),
    }
}
