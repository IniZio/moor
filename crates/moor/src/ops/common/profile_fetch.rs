//! Checks the requester's agent profile out in the Box, at the commit resolved at start.

use std::path::Path;
use std::time::Duration;

use moor_core::box_key::is_machine_handle;
use moor_core::boxscript::profile::{failed_installs, fetch_script, setup_script, FAILURES_FILE};
use moor_core::config::BoxSettings;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::event::{PostCloneStep, ProfileStep};
use moor_core::name::BoxPath;
use moor_core::seam::driver::{ExecOutcome, Place};
use moor_core::{BoxHandle, BoxId, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::activity::box_place;
use crate::ops::common::agent_install::{self, Repeat};
use crate::ops::common::box_exec::{BoxProxy, Site};
use crate::ops::common::dotfiles::{private_log, tail};
use crate::ops::common::setup::log_dir;
use crate::ops::common::step::timed;
use crate::store::box_profile::BoxProfile;
use crate::store::{BoxRow, Store};

/// Wall-clock allowance for the shallow fetch.
const FETCH_DEADLINE: Duration = Duration::from_mins(2);

/// Wall-clock allowance for the profile's own `setup.sh`.
const SETUP_DEADLINE: Duration = agent_install::INSTALL_DEADLINE;

/// Fetches the profile Box `id` resolved, if any, through its Proxy, runs its `setup.sh`, then its
/// plugin installs. The fetch and the installs are safe to repeat, so a lost machine link is waited
/// out; one that failed all the same is recorded, warned and returned as why setup failed, so the
/// agent never starts without the profile. A failed `setup.sh` is recorded and warned only.
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
) -> Result<Result<(), String>, CommandError> {
    let Some(profile) = store.box_profile(id)? else {
        return Ok(Ok(()));
    };
    let proxy = match proxy {
        Ok(proxy) => proxy.as_ref(),
        Err(reason) => {
            return stopped(cx, store, id, ProfileStep::Fetch, vec![reason.clone()]).map(Err);
        }
    };
    let fetched = timed(cx, store, id, PostCloneStep::Profile, || {
        let out = fetch_and_setup(cx, store, (handle, provider), id, (cfg, proxy), &profile);
        let ok = matches!(out, Ok(Ok(true)));
        (out, ok)
    })?;
    if let Err(detail) = fetched {
        return stopped(cx, store, id, ProfileStep::Fetch, vec![detail]).map(Err);
    }
    let site = Site {
        handle,
        provider,
        workdir: None,
    };
    plugin_installs(cx, store, &site, id, (cfg, proxy), &profile)
}

/// Runs the plugin installs of `profile`'s manifest, ahead of the launch, and records each one
/// that failed; the rest still ran. Why setup failed, when one did.
fn plugin_installs(
    cx: &Cx,
    store: &Store,
    site: &Site<'_>,
    id: BoxId,
    env: (&BoxSettings, Option<&BoxProxy>),
    profile: &BoxProfile,
) -> Result<Result<(), String>, CommandError> {
    let manifest = profile.manifest.as_deref();
    let manifest = manifest.and_then(|text| moor_core::profile::parse(text).ok());
    let Some(script) = manifest.and_then(|m| cx.engine().profile_install(&m)) else {
        return Ok(Ok(()));
    };
    let log = cx.state_dir()?;
    let log = log_dir(log, id).join("profile-plugins.log");
    timed(cx, store, id, PostCloneStep::Plugins, || {
        let ran = run(
            cx,
            (site, Place::Agent),
            env,
            (script, &log),
            ("profile plugins", SETUP_DEADLINE, Repeat::Safe),
        );
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        let mut failures: Vec<String> = ran.err().into_iter().collect();
        failures.extend(failed_installs(&text).into_iter().map(str::to_owned));
        if failures.is_empty() {
            return (Ok(Ok(())), true);
        }
        (
            stopped(cx, store, id, ProfileStep::Install, failures).map(Err),
            false,
        )
    })
}

/// Fetches the profile into the Box and runs its `setup.sh` in each place it was fetched to, Work
/// first. Why the fetch failed, unrecorded, or whether every `setup.sh` went well; a place whose
/// setup failed is named in its own event.
fn fetch_and_setup(
    cx: &Cx,
    store: &Store,
    (handle, provider): (&BoxHandle, &str),
    id: BoxId,
    (cfg, proxy): (&BoxSettings, Option<&BoxProxy>),
    profile: &BoxProfile,
) -> Result<Result<bool, String>, CommandError> {
    let url = format!("https://github.com/{}.git", profile.repo);
    let site = Site {
        handle,
        provider,
        workdir: None,
    };
    let script = fetch_script(&url, profile.sha.as_str());
    let places: &[(Place, &str)] = if is_machine_handle(handle.as_str()) {
        &[(Place::Work, "work"), (Place::Agent, "agent")]
    } else {
        &[(Place::Work, "work")]
    };
    let fetched = places.iter().try_for_each(|&(place, name)| {
        let log = cx
            .state_dir()
            .map(|d| {
                d.join("profile-fetch")
                    .join(format!("{}-{name}.log", id.get()))
            })
            .map_err(|e| e.error.to_string())?;
        run(
            cx,
            (&site, place),
            (cfg, proxy),
            (script.clone(), &log),
            ("profile fetch", FETCH_DEADLINE, Repeat::Safe),
        )
    });
    if let Err(detail) = fetched {
        return Ok(Err(detail));
    }
    setups(cx, store, &site, id, (cfg, proxy), places).map(Ok)
}

/// Runs the checkout's `setup.sh` in each of `places` in turn; a place that failed is recorded
/// under its own name and the rest still run. Whether all went well.
fn setups(
    cx: &Cx,
    store: &Store,
    site: &Site<'_>,
    id: BoxId,
    (cfg, proxy): (&BoxSettings, Option<&BoxProxy>),
    places: &[(Place, &str)],
) -> Result<bool, CommandError> {
    let mut all_ok = true;
    for &(place, name) in places {
        let file = if place == Place::Work {
            "profile-setup.log".to_owned()
        } else {
            format!("profile-setup-{name}.log")
        };
        let setup_log = cx
            .state_dir()
            .map(|d| log_dir(d, id).join(file))
            .map_err(|e| e.error.to_string());
        let ran = setup_log.and_then(|log| {
            run(
                cx,
                (site, place),
                (cfg, proxy),
                (setup_script(), &log),
                ("profile setup.sh", SETUP_DEADLINE, Repeat::Once),
            )
        });
        if let Err(detail) = ran {
            let detail = if places.len() > 1 {
                format!("in the {name} place: {detail}")
            } else {
                detail
            };
            failed(cx, store, id, ProfileStep::Setup, detail)?;
            all_ok = false;
        }
    }
    Ok(all_ok)
}

/// Runs `script` in `place` in the Box, logging to `log`, asked again across a lost machine
/// link when `repeat` allows; the error says why it failed.
fn run(
    cx: &Cx,
    (site, place): (&Site<'_>, Place),
    (cfg, proxy): (&BoxSettings, Option<&BoxProxy>),
    (script, log): (String, &Path),
    (what, deadline, repeat): (&str, Duration, Repeat),
) -> Result<(), String> {
    private_log(log).map_err(|e| format!("could not create the log: {e}"))?;
    let limit = (what, deadline);
    let reason = match agent_install::run(cx, site, place, cfg, proxy, (script, repeat), log, limit)
    {
        Ok(ExecOutcome::Exited { code: 0 }) => return Ok(()),
        Ok(ExecOutcome::Exited { code }) => format!("exit {code}"),
        Ok(ExecOutcome::Interrupted) => "interrupted".to_owned(),
        Err(e) => e,
    };
    let logged = std::fs::read_to_string(log)
        .map(|s| tail(&s))
        .unwrap_or_default();
    Err(if logged.is_empty() {
        reason
    } else {
        format!("{reason}\n{logged}")
    })
}

/// What went wrong with the profile at `step`.
const fn what(step: ProfileStep) -> &'static str {
    match step {
        ProfileStep::Resolve => "could not be resolved",
        ProfileStep::Manifest => "has a bad manifest or marketplace",
        ProfileStep::Fetch => "could not be fetched into the Box",
        ProfileStep::Install => "was only partly installed",
        ProfileStep::Setup => "had a setup.sh that did not finish",
    }
}

/// Records a failed profile `step` on Box `id` and warns, so the thread shows it; the Box goes on
/// without what the step would have given it.
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
    let after = if matches!(step, ProfileStep::Install | ProfileStep::Setup) {
        "the agent runs without that part"
    } else {
        "the Box starts without it"
    };
    report(cx, store, id, (step, detail), after)
}

/// Records each of `details` as a failed profile `step` on Box `id` and warns that setup failed;
/// the cause setup failed with.
///
/// # Errors
/// [`CommandError`] when an event cannot be written.
fn stopped(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    step: ProfileStep,
    details: Vec<String>,
) -> Result<String, CommandError> {
    let cause = format!("agent profile: {}: {}", what(step), details.join("; "));
    for detail in details {
        let after = "setup failed, so the agent does not start without it";
        report(cx, store, id, (step, detail), after)?;
    }
    Ok(cause)
}

/// Writes the `profile_failed` event and its warning, which ends with `after`.
fn report(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    (step, detail): (ProfileStep, String),
    after: &str,
) -> Result<(), CommandError> {
    cx.warn(Warning {
        code: "profile_failed".to_owned(),
        message: format!("your agent profile {}: {detail}; {after}", what(step)),
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
    let Some(place) = box_place(row) else {
        return Ok(());
    };
    let path = BoxPath::under(place.1.as_str(), FAILURES_FILE);
    let read = path.map_err(|e| e.to_string()).and_then(|p| {
        cx.driver()
            .peek_file(&place.0, Place::Agent, &p)
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
