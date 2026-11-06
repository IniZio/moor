//! Suspends and resumes a Box through `crabbox pause` and `crabbox resume`.
//!
//! Both commands find the Box from `--id` and the local lease claim, so they run from the
//! lease dir, as the warmup did.

use std::time::Duration;

use moor_core::name::{BoxHandle, ProviderId};
use moor_core::seam::driver::{Driver, DriverError, PausePublish, SleepKind, SleepOutcome};
use moor_core::seam::proc::ProcSpec;

use super::errors::Op;
use super::{split_handle, target_args, Crabbox};

/// Time allowed for one pause or resume.
const PAUSE_DEADLINE: Duration = Duration::from_mins(2);

/// Asks crabbox's pause to save the lease's caches.
const PAUSE_PUBLISH: &str = "CRABBOX_PAUSE_PUBLISH";

/// Reason given when no provider is named in the handle.
const NO_PROVIDER: &str = "the box has no known provider, so moor cannot tell how it sleeps";

/// Sleeps the Box as its provider's facts allow.
pub(super) fn sleep(
    cb: &Crabbox,
    handle: &BoxHandle,
    publish: PausePublish,
) -> Result<SleepOutcome, DriverError> {
    if super::pool::is_pool(handle) {
        return super::pool::sleep(cb, handle, publish);
    }
    let Some(provider) = split_handle(handle).0 else {
        return Ok(SleepOutcome::CannotSleep(NO_PROVIDER.to_owned()));
    };
    let caps = match cb.caps(&ProviderId::new(provider)) {
        Ok(caps) => caps,
        Err(DriverError::UnknownProvider(_)) => {
            return Ok(SleepOutcome::CannotSleep(format!(
                "provider {provider} is unknown; box stays up"
            )));
        }
        Err(e) => return Err(e),
    };
    match caps.sleep {
        SleepKind::Native => Ok(SleepOutcome::Native),
        SleepKind::None => Ok(SleepOutcome::CannotSleep(format!(
            "provider {provider} cannot sleep a box"
        ))),
        SleepKind::Suspend if !caps.wake => Ok(SleepOutcome::CannotSleep(format!(
            "provider {provider} cannot wake a sleeping box"
        ))),
        SleepKind::Suspend => {
            let spec = spec(cb, "pause", handle)?;
            let spec = match publish {
                PausePublish::Allowed => spec.env(PAUSE_PUBLISH, "1"),
                PausePublish::Denied => spec,
            };
            cb.call(Op::Sleep, split_handle(handle).0, &spec)?;
            Ok(SleepOutcome::Slept)
        }
    }
}

/// Wakes the Box as its provider's facts allow; a native sleeper needs no call.
pub(super) fn wake(cb: &Crabbox, handle: &BoxHandle) -> Result<(), DriverError> {
    if super::pool::is_pool(handle) {
        return super::pool::wake(cb, handle);
    }
    let Some(provider) = split_handle(handle).0 else {
        return Err(DriverError::Unsupported(
            "box has no known provider to wake",
        ));
    };
    match cb.caps(&ProviderId::new(provider))?.sleep {
        SleepKind::Native => Ok(()),
        SleepKind::Suspend => {
            let spec = spec(cb, "resume", handle)?;
            cb.call(Op::Wake, split_handle(handle).0, &spec).map(|_| ())
        }
        SleepKind::None => Err(DriverError::Unsupported("provider cannot wake a box")),
    }
}

/// The `crabbox <verb>` call for the Box.
fn spec(cb: &Crabbox, verb: &str, handle: &BoxHandle) -> Result<ProcSpec, DriverError> {
    let spec = Crabbox::spec(PAUSE_DEADLINE)
        .arg(verb)
        .args(target_args(handle));
    cb.at_lease(spec, handle)
}

#[cfg(test)]
mod tests;
