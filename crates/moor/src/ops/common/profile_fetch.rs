//! Checks the requester's agent profile out in the Box, at the commit resolved at start.

use std::time::Duration;

use moor_core::boxscript::profile::fetch_script;
use moor_core::config::BoxSettings;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::event::ProfileStep;
use moor_core::name::BoxPath;
use moor_core::seam::driver::ExecOutcome;
use moor_core::{BoxHandle, BoxId, Warning};

use crate::adapters::box_agent::FAILURES_FILE;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::activity::session_ref;
use crate::ops::common::agent_install;
use crate::ops::common::box_exec::{BoxProxy, Site};
use crate::ops::common::dotfiles::{private_log, tail};
use crate::store::{BoxRow, Store};

/// Wall-clock allowance for the shallow fetch.
const FETCH_DEADLINE: Duration = Duration::from_mins(2);

/// Fetches the profile Box `id` resolved, if any, through its Proxy; a failure is recorded and
/// warned, never returned.
///
/// # Errors
/// [`CommandError`] when the store cannot be read or written.
pub(crate) fn apply(
    cx: &Cx,
    store: &Store,
    (handle, provider): (&BoxHandle, &str),
    id: BoxId,
    cfg: &BoxSettings,
    proxy: &Result<Option<BoxProxy>, String>,
) -> Result<(), CommandError> {
    let Some(profile) = store.box_profile(id)? else {
        return Ok(());
    };
    let proxy = match proxy {
        Ok(proxy) => proxy.as_ref(),
        Err(reason) => return failed(cx, store, id, ProfileStep::Fetch, reason.clone()),
    };
    let url = format!("https://github.com/{}.git", profile.repo);
    let site = Site {
        handle,
        provider,
        workdir: None,
    };
    let script = fetch_script(&url, profile.sha.as_str());
    match run(cx, &site, cfg, proxy, script, id) {
        Ok(()) => Ok(()),
        Err(detail) => failed(cx, store, id, ProfileStep::Fetch, detail),
    }
}

/// Runs the fetch `script` in the Box, logging under the state dir; the error says why it failed.
fn run(
    cx: &Cx,
    site: &Site<'_>,
    cfg: &BoxSettings,
    proxy: Option<&BoxProxy>,
    script: String,
    id: BoxId,
) -> Result<(), String> {
    let log = cx
        .state_dir()
        .map_err(|e| e.error.to_string())?
        .join("profile-fetch")
        .join(format!("{}.log", id.get()));
    private_log(&log).map_err(|e| format!("could not create the fetch log: {e}"))?;
    let reason = match agent_install::run(cx, site, cfg, proxy, script, &log, FETCH_DEADLINE) {
        Ok(ExecOutcome::Exited { code: 0 }) => return Ok(()),
        Ok(ExecOutcome::Exited { code }) => format!("exit {code}"),
        Ok(ExecOutcome::Interrupted) => "interrupted".to_owned(),
        Err(e) => e,
    };
    let logged = std::fs::read_to_string(&log)
        .map(|s| tail(&s))
        .unwrap_or_default();
    Err(if logged.is_empty() {
        reason
    } else {
        format!("{reason}\n{logged}")
    })
}

/// Records a failed profile `step` on Box `id` and warns, so the thread shows it.
///
/// # Errors
/// [`CommandError`] when the event cannot be written.
pub(crate) fn failed(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    step: ProfileStep,
    detail: String,
) -> Result<(), CommandError> {
    let what = match step {
        ProfileStep::Resolve => "could not be resolved",
        ProfileStep::Manifest => "has a bad manifest or marketplace",
        ProfileStep::Fetch => "could not be fetched into the Box",
        ProfileStep::Install => "was only partly installed",
    };
    let after = if step == ProfileStep::Install {
        "the agent runs without that part"
    } else {
        "the Box starts without it"
    };
    cx.warn(Warning {
        code: "profile_failed".to_owned(),
        message: format!("your agent profile {what}: {detail}; {after}"),
    });
    store.apply(Decision::new(id).event(ExtraEvent::ProfileFailed { step, detail }))?;
    Ok(())
}

/// Records each line of the launch's install failures file in Box `row` as a failed install.
/// A Box with no stored profile, or no such file, records nothing; a failed read is warned.
///
/// # Errors
/// [`CommandError`] when the store cannot be read or written.
pub(crate) fn installs(cx: &Cx, store: &Store, row: &BoxRow) -> Result<(), CommandError> {
    if store.box_profile(row.id)?.is_none() {
        return Ok(());
    }
    let Some(session) = session_ref(row, String::new(), None) else {
        return Ok(());
    };
    let path = BoxPath::under(session.workdir.as_str(), FAILURES_FILE);
    let read = path.map_err(|e| e.to_string()).and_then(|p| {
        cx.driver()
            .peek_file(&session.handle, &p)
            .map_err(|e| e.to_string())
    });
    let text = match read {
        Ok(text) => text.unwrap_or_default(),
        Err(e) => {
            cx.warn(Warning {
                code: "profile_check_failed".to_owned(),
                message: format!("could not read which profile installs failed: {e}"),
            });
            return Ok(());
        }
    };
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        failed(cx, store, row.id, ProfileStep::Install, line.to_owned())?;
    }
    Ok(())
}
