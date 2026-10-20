//! Composition root: the single entry every caller uses.

use crate::cx::Cx;
use crate::env::Env;
use crate::error::CommandError;
use crate::ops;
use crate::wire;
use moor_core::{ArtifactReadReq, Request, Response};

/// Wired application: `call` answers a request, `read_artifact` also streams bytes.
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

    /// Wrap an already built context, as tests do over fakes.
    #[cfg(test)]
    pub(crate) const fn from_cx(cx: Cx) -> Self {
        Self { cx }
    }

    /// Opens the state store once; the error is the one every command would get.
    ///
    /// # Errors
    /// A refusal for a state dir at another schema version, else the store or clock failure.
    pub fn open_store(env: &Env) -> Result<(), CommandError> {
        wire::open_store(env)
    }

    /// Settles the Proxy supervisors recorded in the state store; one line per failure.
    #[must_use]
    pub fn reconcile_proxies(env: &Env) -> Vec<String> {
        wire::reconcile_proxies(env)
    }

    /// The `version` envelope; independent of config and state.
    #[must_use]
    pub fn version() -> Response {
        match ops::version_value() {
            Ok(value) => Response::ok("version", Some(value)),
            Err(e) => e.into_response("version", vec![]),
        }
    }

    /// Run one request to an envelope; never panics, never prints.
    pub fn call(&self, req: Request) -> Response {
        let cmd = req.cmd();
        match ops::dispatch(&self.cx, req) {
            Ok(value) => Response::ok(cmd, Some(value)).with_warnings(self.cx.take_warnings()),
            Err(e) => e.into_response(cmd, self.cx.take_warnings()),
        }
    }

    /// Run `artifact-read`, passing the Artifact's bytes to `out` piece by piece, then answer
    /// with what was sent.
    pub fn read_artifact(
        &self,
        req: &ArtifactReadReq,
        out: &mut dyn FnMut(&[u8]) -> std::io::Result<()>,
    ) -> Response {
        let cmd = "artifact-read";
        match ops::read_artifact(&self.cx, req, out) {
            Ok(value) => Response::ok(cmd, Some(value)).with_warnings(self.cx.take_warnings()),
            Err(e) => e.into_response(cmd, self.cx.take_warnings()),
        }
    }
}

mod feed;

#[cfg(test)]
mod tests;
