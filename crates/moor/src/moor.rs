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

    /// Wrap an already built context, as tests do over fakes.
    #[cfg(test)]
    pub(crate) const fn from_cx(cx: Cx) -> Self {
        Self { cx }
    }

    /// Settles the Proxy supervisors recorded in the state store; one line per failure.
    #[must_use]
    pub fn reconcile_proxies(env: &Env) -> Vec<String> {
        wire::reconcile_proxies(env)
    }

    /// The web URL of the open pull request on the live Task `task`'s branch, looked up through the forge on this host.
    pub fn open_pr_url(env: &Env, task: &str) -> Result<Option<String>, String> {
        let cx = wire::build(env).map_err(|e| e.error.to_string())?;
        ops::pr::open_pr_url(&cx, task)
    }

    /// The `version` envelope; independent of config and state.
    #[must_use]
    pub fn version() -> Response {
        match ops::version_value() {
            Ok(value) => Response::ok("version", Some(value)),
            Err(e) => e.into_response("version", vec![]),
        }
    }

    /// Names of the repos the controller knows, sorted.
    pub(crate) fn known_repo_names(&self) -> Vec<String> {
        self.cx.known_repos().keys().cloned().collect()
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

mod feed;

#[cfg(test)]
mod tests;
