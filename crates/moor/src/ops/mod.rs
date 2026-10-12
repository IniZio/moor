//! Command operations: one op per request.

use moor_core::{ErrorKind, Request};

use crate::cx::Cx;
use crate::error::CommandError;

mod close;
pub(crate) mod common;
pub(crate) mod env;
mod show;
mod start;
mod version;

#[cfg(test)]
mod tests;

/// Route a request to its op.
pub(crate) fn dispatch(cx: &Cx, req: Request) -> Result<serde_json::Value, CommandError> {
    match req {
        Request::Version {} => version_value(),
        Request::Start(r) => to_value(&start::run(cx, &r)?),
        Request::Show(r) => to_value(&show::run(cx, &r)?),
        Request::Close(r) => to_value(&close::run(cx, &r)?),
        Request::Env(r) => to_value(&env::run(cx, &r)?),
        other => Err(CommandError::not_implemented(other.cmd())),
    }
}

/// The `version` result; needs no context, so it works without config.
pub(crate) fn version_value() -> Result<serde_json::Value, CommandError> {
    serde_json::to_value(version::exec()).map_err(|e| CommandError::new(ErrorKind::Internal, e))
}

/// Serialise a view into the response value.
fn to_value<T: serde::Serialize>(view: &T) -> Result<serde_json::Value, CommandError> {
    serde_json::to_value(view).map_err(|e| CommandError::new(ErrorKind::Internal, e))
}
