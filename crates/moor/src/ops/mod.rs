//! Command operations: one op per request.

use moor_core::{ErrorKind, Request};

use crate::cx::Cx;
use crate::error::CommandError;

mod close;
pub(crate) mod common;
mod cp;
pub(crate) mod env;
mod gate;
mod open;
mod run;
mod show;
mod sleep;
mod start;
mod version;
mod wait;
mod wake;

#[cfg(test)]
mod tests;

/// Route a request to its op.
pub(crate) fn dispatch(cx: &Cx, req: Request) -> Result<serde_json::Value, CommandError> {
    match req {
        Request::Version {} => version_value(),
        Request::Start(r) => to_value(&start::run(cx, &r)?),
        Request::Show(r) => to_value(&show::run(cx, &r)?),
        Request::Close(r) => to_value(&close::run(cx, &r)?),
        Request::Gate(r) => to_value(&gate::run(cx, &r)?),
        Request::Env(r) => to_value(&env::run(cx, &r)?),
        Request::Run(r) => to_value(&run::run(cx, &r)?),
        Request::Wait(r) => to_value(&wait::run(cx, &r)?),
        Request::Cp(r) => to_value(&cp::run(cx, &r)?),
        Request::Sleep(r) => to_value(&sleep::run(cx, &r)?),
        Request::Wake(r) => to_value(&wake::run(cx, &r)?),
        Request::Open(r) => to_value(&open::run(cx, &r)?),
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
