//! Composition root: the single entry every caller uses.

use std::path::{Path, PathBuf};

use crate::cx::Cx;
use crate::env::Env;
use crate::error::CommandError;
use crate::ops;
use crate::vault::Vault;
use crate::wire;
use moor_core::request::{Grant, UserCmd};
use moor_core::seam::checkout::Checkout;
use moor_core::user::{Caller, Identity, Viewer};
use moor_core::{ArtifactReadReq, BoxId, ErrorKind, Request, Response, RunReq};

/// Wired application: `call` answers a request; `read_artifact` and `run_streamed` also stream
/// bytes.
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

    /// The caller's git checkout, read with the host's git, for the CLI to resolve its repo.
    #[must_use]
    pub fn host_checkout(env: &Env) -> impl Checkout {
        wire::host_checkout(env)
    }

    /// Wrap an already built context, as tests do over fakes.
    #[cfg(test)]
    pub(crate) const fn from_cx(cx: Cx) -> Self {
        Self { cx }
    }

    /// Opens the state store once and returns `env` sharing it with every request and tick; the
    /// error is the one every command would get.
    ///
    /// # Errors
    /// A refusal for a state dir at another schema version, else the store or clock failure.
    pub(crate) fn open_store(env: Env) -> Result<Env, CommandError> {
        wire::open_store(env)
    }

    /// Ends every front Proxy lease: a controller that holds the state lock is the only one,
    /// and it restarts every front that held a lease.
    ///
    /// # Errors
    /// The store or clock failure.
    pub(crate) fn end_front_leases(env: &Env) -> Result<(), CommandError> {
        wire::end_front_leases(env)
    }

    /// The socket of front `ns`'s Proxy for the state dir `state_dir`, which its shim bridges.
    pub(crate) fn front_proxy_socket(state_dir: &Path, ns: &str) -> PathBuf {
        wire::front_proxy_socket(state_dir, ns)
    }

    /// Creates the directory of front `ns`'s Proxy socket, which its shim opens at start.
    ///
    /// # Errors
    /// Why the directory could not be made or is not private.
    pub(crate) fn prepare_front_proxy_socket(state_dir: &Path, ns: &str) -> Result<(), String> {
        wire::prepare_front_proxy_socket(state_dir, ns)
    }

    /// The CA certificate the front Proxy signs with, made when missing.
    ///
    /// # Errors
    /// Why the CA could not be read or made.
    pub(crate) fn front_proxy_ca(state_dir: &Path) -> std::io::Result<PathBuf> {
        wire::front_proxy_ca(state_dir)
    }

    /// Settles the Proxy supervisors recorded in the state store; one line per failure.
    #[must_use]
    pub fn reconcile_proxies(env: &Env) -> Vec<String> {
        wire::reconcile_proxies(env)
    }

    /// Leaves SIGINT and SIGTERM to the controller for the life of the process, so a streamed
    /// command ends only on its own or at its deadline. False when a stream already took them.
    pub(crate) fn leave_signals_to_controller() -> bool {
        wire::leave_signals_to_controller()
    }

    /// The `version` envelope; independent of config and state.
    #[must_use]
    pub fn version() -> Response {
        match ops::version_value() {
            Ok(value) => Response::ok("version", Some(value)),
            Err(e) => e.into_response("version", vec![]),
        }
    }

    /// This engine acting as `who`, an identity its socket already checked.
    #[must_use]
    pub(crate) fn acting_as(self, who: Identity) -> Self {
        Self {
            cx: self.cx.with_requester(who),
        }
    }

    /// This engine seeing only the Boxes `viewer` may see.
    #[must_use]
    pub(crate) fn viewing_as(self, viewer: Viewer) -> Self {
        Self {
            cx: self.cx.with_viewer(viewer),
        }
    }

    /// The command that relays the agent's terminal on the Box `req` names.
    ///
    /// # Errors
    /// [`CommandError`] when the Box is not open, asleep, or has no machine to reach.
    pub(crate) fn attach_argv(
        &self,
        req: &moor_core::AttachReq,
    ) -> Result<Vec<String>, CommandError> {
        ops::attach::argv(&self.cx, req)
    }

    /// This engine doing only what `grant` lets its request do.
    #[must_use]
    pub(crate) fn granted(self, grant: Grant) -> Self {
        Self {
            cx: self.cx.with_grant(grant),
        }
    }

    /// Refuses `request` unless `caller`, asserting `asserted`, may act on every Box it names;
    /// returns what the request may see.
    ///
    /// # Errors
    /// The refusal or store failure the request answers with instead.
    pub(crate) fn authorize(
        &self,
        caller: &Result<Caller, String>,
        asserted: Option<&Identity>,
        request: &Request,
    ) -> Result<Grant, CommandError> {
        ops::authz::authorize(&self.cx, caller, asserted, request)
    }

    /// Whose Boxes a watch from `caller` asserting `asserted` may see; no Box when `caller` may
    /// not assert it or that cannot be told.
    pub(crate) fn watcher(
        &self,
        caller: &Result<Caller, String>,
        asserted: Option<&Identity>,
    ) -> Viewer {
        ops::authz::viewer(&self.cx, caller, asserted).unwrap_or(Viewer::Nobody)
    }

    /// Whether this engine's viewer may see Box `id`.
    ///
    /// # Errors
    /// The store failure, as text.
    pub(crate) fn may_see(&self, id: BoxId) -> Result<bool, String> {
        let store = self.cx.store().map_err(|e| e.error.to_string())?;
        ops::authz::may_see(&self.cx, store, id).map_err(|e| e.error.to_string())
    }

    /// Run one request to an envelope; never panics, never prints.
    pub fn call(&self, req: Request) -> Response {
        let cmd = req.cmd();
        match ops::dispatch(&self.cx, req) {
            Ok(value) => Response::ok(cmd, Some(value)).with_warnings(self.cx.take_warnings()),
            Err(e) => e.into_response(cmd, self.cx.take_warnings()),
        }
    }

    /// Run a user request as the identity `caller` may assert, `asserted` or its own; any
    /// other identity is refused.
    pub(crate) fn call_user(
        &self,
        caller: &Result<Caller, String>,
        asserted: Option<&Identity>,
        vault: &Vault,
        cmd: &UserCmd,
    ) -> Response {
        let name = cmd.cmd();
        let who = caller
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|caller| {
                if matches!(
                    cmd,
                    UserCmd::Enroll { email: Some(_) }
                        | UserCmd::MachineEnroll { email: Some(_), .. }
                ) && !matches!(caller, Caller::Front(_))
                {
                    return Err("only a front can vouch for an email".to_owned());
                }
                caller.resolve(asserted)
            })
            .map_err(|why| CommandError::new(ErrorKind::Refused, why));
        match who.and_then(|who| ops::users::run(&self.cx, vault, &who, cmd)) {
            Ok(value) => Response::ok(name, Some(value)).with_warnings(self.cx.take_warnings()),
            Err(e) => e.into_response(name, self.cx.take_warnings()),
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

    /// Run `run`, passing the command's output to `out` piece by piece as it reaches the run
    /// log, then answer.
    pub fn run_streamed(
        &self,
        req: &RunReq,
        out: &mut (dyn FnMut(&[u8]) -> std::io::Result<()> + Send),
    ) -> Response {
        let cmd = "run";
        match ops::run_streamed(&self.cx, req, out) {
            Ok(value) => Response::ok(cmd, Some(value)).with_warnings(self.cx.take_warnings()),
            Err(e) => e.into_response(cmd, self.cx.take_warnings()),
        }
    }
}

mod feed;

#[cfg(test)]
mod tests;
