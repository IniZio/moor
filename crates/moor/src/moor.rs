//! Composition root: the single entry every caller uses.

use std::sync::Arc;

use crate::adapters::clock::SystemClock;
use crate::cx::Cx;
use crate::env::Env;
use crate::error::CommandError;
use crate::ops;
use moor_core::{ErrorKind, Request, Response};

/// Wired application; `call` is its only operation.
pub struct Moor {
    /// Per-process context handed to every op.
    cx: Cx,
}

impl Moor {
    /// Build from a captured environment.
    pub fn from_env(env: &Env) -> Result<Self, CommandError> {
        let clock =
            SystemClock::from_env(env).map_err(|e| CommandError::new(ErrorKind::Usage, e))?;
        Ok(Self {
            cx: Cx::new(env.state_dir(), Arc::new(clock)),
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
