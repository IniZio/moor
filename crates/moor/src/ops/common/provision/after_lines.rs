//! The steps after a Box's setup lines: its dotfiles, agent install and profile.

use moor_core::config::BoxSettings;
use moor_core::{BoxHandle, BoxId, Warning};

use super::clone::SetupProxy;
use super::outcome::{dotfiles_warned, AfterFailed};
use super::SetupTarget;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::dotfiles::{self, DotfilesRun};
use crate::ops::common::setup::SetupRun;
use crate::ops::common::{agent_install, profile_fetch};
use crate::store::Store;

/// Runs what comes after the setup lines. A Box failing a line gets no agent, so the steps that
/// prepare one wait for the retry.
pub(super) fn steps_after_lines(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    target: &SetupTarget<'_>,
    (cfg, proxy): (&BoxSettings, &SetupProxy),
    run: &SetupRun,
) -> Result<(DotfilesRun, Result<(), AfterFailed>), CommandError> {
    if run.failed() {
        return Ok((DotfilesRun::Skipped, Ok(())));
    }
    let at = (target.handle, target.provider);
    let after = after_setup(cx, store, at, id, cfg, proxy, true)?;
    Ok((after.dotfiles, after.installed))
}

/// How the steps after the setup lines went.
pub(super) struct AfterSetup {
    /// The dotfiles.
    pub(super) dotfiles: DotfilesRun,
    /// The agent install and the profile, with which failed and why.
    pub(super) installed: Result<(), AfterFailed>,
}

/// Applies the dotfiles; a failure is warned and reported, and the Box carries on.
fn apply_dotfiles(
    cx: &Cx,
    store: &Store,
    at: (&BoxHandle, &str),
    id: BoxId,
    cfg: &BoxSettings,
    proxy: &SetupProxy,
) -> DotfilesRun {
    match dotfiles::apply(cx, store, at, id, cfg, proxy) {
        Ok(d) => d,
        Err(f) => dotfiles_warned(cx, f),
    }
}

/// Lets the Box's login pick up the groups setup added.
fn refresh_login(cx: &Cx, handle: &BoxHandle) {
    if let Err(e) = cx.driver().refresh_login(handle) {
        cx.warn(Warning {
            code: "login_not_refreshed".to_owned(),
            message: format!(
                "could not check the Box login for groups setup added: {e}; the agent may lack them"
            ),
        });
    }
}

/// Runs the steps after the setup lines, the agent install only when `install`. A failed agent
/// install is warned and returned as the Box's failed step, and the profile, which needs the
/// agent, waits for the retry. A profile that could not be fetched or installed is the failed
/// step too, so the agent never starts without it.
#[allow(
    clippy::too_many_arguments,
    reason = "the Box, settings, Proxy and whether to install"
)]
pub(super) fn after_setup(
    cx: &Cx,
    store: &Store,
    at: (&BoxHandle, &str),
    id: BoxId,
    cfg: &BoxSettings,
    proxy: &SetupProxy,
    install: bool,
) -> Result<AfterSetup, CommandError> {
    let dotfiles = apply_dotfiles(cx, store, at, id, cfg, proxy);
    let installed = if install {
        agent_install::apply(cx, store, at, id, cfg, proxy)
    } else {
        Ok(())
    };
    if let Err(reason) = installed {
        cx.warn(agent_install::warning(&reason));
        return Ok(AfterSetup {
            dotfiles,
            installed: Err(AfterFailed {
                step: "agent_install",
                cause: format!("agent install: {reason}"),
            }),
        });
    }
    if let Err(cause) = profile_fetch::apply(cx, store, at, id, cfg, proxy)? {
        return Ok(AfterSetup {
            dotfiles,
            installed: Err(AfterFailed {
                step: "profile",
                cause,
            }),
        });
    }
    refresh_login(cx, at.0);
    Ok(AfterSetup {
        dotfiles,
        installed: Ok(()),
    })
}
