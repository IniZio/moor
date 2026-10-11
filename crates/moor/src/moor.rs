//! Composition root: the single entry every caller uses.

use crate::cx::Cx;
use crate::env::Env;
use crate::error::CommandError;
use crate::ops;
use crate::wire;
use moor_core::{Request, Response};

/// Wired application; `call` is its only operation.
pub struct Moor {
    /// Per-process context handed to every op.
    cx: Cx,
}

impl Moor {
    /// Build from a captured environment.
    pub fn from_env(env: &Env) -> Result<Self, CommandError> {
        Ok(Self {
            cx: wire::build(env)?,
        })
    }

    /// Run one request to an envelope; never panics, never prints.
    pub fn call(&self, req: Request) -> Response {
        let cmd = req.cmd();
        match ops::dispatch(&self.cx, req) {
            Ok(value) => Response::ok(cmd, Some(value)).with_warnings(self.cx.take_warnings()),
            Err(e) => e.into_response(cmd, self.cx.take_warnings()),
        }
    }
}

#[cfg(test)]
mod tests;
