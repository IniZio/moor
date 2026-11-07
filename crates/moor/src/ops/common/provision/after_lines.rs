//! The steps after a Box's setup lines: its dotfiles, agent install and profile, and the checkpoint
//! that holds them.

use moor_core::config::BoxSettings;
use moor_core::domain::layers::SLOW_LINE_MS;
use moor_core::{BoxHandle, BoxId, Warning};

use super::clone::SetupProxy;
use super::layers::{after_finishing, roll_forward, Layered};
use super::outcome::dotfiles_warned;
use super::SetupTarget;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::dotfiles::{self, DotfilesRun};
use crate::ops::common::setup::SetupRun;
use crate::ops::common::{agent_install, profile_fetch};
use crate::store::Store;

/// Runs what comes after the setup lines, and checkpoints it once it all went well. A Box failing a
/// line gets no agent, so the steps that prepare one wait for the retry. A fork that holds those
/// steps runs none of them, unless a rerun line or a lost cache took its agent.
pub(super) fn steps_after_lines(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    (on_fork, target, at_base): (&SetupTarget<'_>, &SetupTarget<'_>, bool),
    (cfg, proxy): (&BoxSettings, &SetupProxy),
    run: &SetupRun,
) -> Result<(DotfilesRun, Result<(), String>), CommandError> {
    if run.failed() {
        return Ok((DotfilesRun::Skipped, Ok(())));
    }
    let at = (target.handle, target.provider);
    let rolling = on_fork.layered.filter(|l| at_base && l.may_roll());
    let slow = run.ms().is_some_and(|ms| ms > SLOW_LINE_MS);
    let holds_steps = on_fork.layered.is_some_and(Layered::finished);
    if holds_steps && agent_install::present(cx, at, id, cfg, proxy) {
        refresh_login(cx, at.0);
        if let Some(l) = rolling.filter(|_| slow) {
            roll_forward(cx, store, on_fork, l, l.above());
        }
        return Ok((DotfilesRun::Skipped, Ok(())));
    }
    // A disk that holds the steps above its top line can never be saved as that line.
    if let Some(l) = rolling.filter(|l| !holds_steps && (slow || !l.top_kept(store))) {
        roll_forward(cx, store, on_fork, l, l.top());
    }
    let began = cx.clock().instant();
    let after = after_setup(cx, store, at, id, cfg, proxy)?;
    if after.went_well() {
        let spent = cx.clock().instant().saturating_duration_since(began);
        let ms = u64::try_from(spent.as_millis()).unwrap_or(u64::MAX);
        match rolling {
            Some(l) => roll_forward(cx, store, on_fork, l, l.above()),
            None => after_finishing(cx, store, target, ms),
        }
    }
    Ok((after.dotfiles, after.installed))
}

/// How the steps after the setup lines went.
pub(super) struct AfterSetup {
    /// The dotfiles.
    pub(super) dotfiles: DotfilesRun,
    /// The agent install, with why it failed.
    pub(super) installed: Result<(), String>,
    /// Whether the profile applied in full.
    profile_ok: bool,
}

impl AfterSetup {
    /// Whether a checkpoint may hold what these steps left.
    const fn went_well(&self) -> bool {
        self.profile_ok
            && self.installed.is_ok()
            && !matches!(self.dotfiles, DotfilesRun::Failed { .. })
    }
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

/// Runs the steps after the setup lines. A failed agent install is warned and returned as the
/// Box's failed step, and the profile, which needs the agent, waits for the retry.
pub(super) fn after_setup(
    cx: &Cx,
    store: &Store,
    at: (&BoxHandle, &str),
    id: BoxId,
    cfg: &BoxSettings,
    proxy: &SetupProxy,
) -> Result<AfterSetup, CommandError> {
    let dotfiles = apply_dotfiles(cx, store, at, id, cfg, proxy);
    if let Err(reason) = agent_install::apply(cx, store, at, id, cfg, proxy) {
        cx.warn(agent_install::warning(&reason));
        return Ok(AfterSetup {
            dotfiles,
            installed: Err(format!("agent install: {reason}")),
            profile_ok: false,
        });
    }
    let profile_ok = profile_fetch::apply(cx, store, at, id, cfg, proxy)?;
    refresh_login(cx, at.0);
    Ok(AfterSetup {
        dotfiles,
        installed: Ok(()),
        profile_ok,
    })
}
