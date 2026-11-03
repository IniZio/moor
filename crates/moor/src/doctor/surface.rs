//! The surface check: what holds each Box agent's pane.

use moor_core::config::SurfaceKind;
use moor_core::seam::proc::{Deadline, ProcError, ProcSpec, Tool};

use super::{Check, Inputs};
use crate::wire::{herdr_env, surface_kind};

/// Ceiling for the herdr probe.
const DEADLINE_SECS: u64 = 10;

/// The fix `docs/config.md` gives for a surface that does not work.
const FIX: &str = "install herdr, or set surface.kind to pty with a moor built with pty-surface";

/// Whether the configured surface can hold panes on this host.
pub(super) fn check(i: &Inputs<'_>) -> Check {
    let kind = match surface_kind(i.config, i.env) {
        Ok(kind) => kind,
        Err(e) => return Check::fail("surface", e.error.to_string(), FIX),
    };
    match kind {
        SurfaceKind::Pty if cfg!(feature = "pty-surface") => {
            Check::ok("surface", "pty, built with pty-surface")
        }
        SurfaceKind::Pty => Check::fail("surface", "pty, but this moor lacks pty-surface", FIX),
        SurfaceKind::Herdr => herdr(i),
    }
}

/// Asks herdr for its workspaces; reading never starts the server.
fn herdr(i: &Inputs<'_>) -> Check {
    let session = i.config.herdr_session();
    let spec = ProcSpec::new(
        Tool::Herdr,
        Deadline(std::time::Duration::from_secs(DEADLINE_SECS)),
    )
    .args(["workspace", "list"])
    .env("HERDR_SESSION", session);
    let spec = herdr_env(i.env)
        .iter()
        .fold(spec, |s, (k, v)| s.env(k.as_str(), v.as_str()));
    match i.proc.run(&spec) {
        Ok(_) => Check::ok("surface", format!("herdr session {session} answers")),
        Err(ProcError::Spawn(_) | ProcError::NonZero { code: 127, .. }) => {
            Check::fail("surface", "herdr is not on PATH", FIX)
        }
        Err(ProcError::NonZero {
            stdout,
            stderr_tail,
            ..
        }) if format!("{stdout}{stderr_tail}").contains("server_not_running") => Check::warn(
            "surface",
            format!("herdr session {session} is not running"),
            "the controller starts it with the first Box",
        ),
        Err(e) => Check::fail("surface", format!("herdr: {e}"), FIX),
    }
}
