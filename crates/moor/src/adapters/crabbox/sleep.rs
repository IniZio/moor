//! Suspends and resumes a Box through `crabbox pause` and `crabbox resume`.
//!
//! Both commands find the Box from `--id` and the local lease claim, so they run from the
//! lease dir, as the warmup did.

use std::time::Duration;

use moor_core::name::{BoxHandle, ProviderId};
use moor_core::seam::driver::{Driver, DriverError, SleepKind, SleepOutcome};

use super::errors::Op;
use super::{split_handle, target_args, Crabbox};

/// Time allowed for one pause or resume.
const PAUSE_DEADLINE: Duration = Duration::from_mins(2);

/// Reason given when no provider is named in the handle.
const NO_PROVIDER: &str = "the box has no known provider, so moor cannot tell how it sleeps";

/// Sleeps the Box as its provider's facts allow.
pub(super) fn sleep(cb: &Crabbox, handle: &BoxHandle) -> Result<SleepOutcome, DriverError> {
    let Some(provider) = split_handle(handle).0 else {
        return Ok(SleepOutcome::CannotSleep(NO_PROVIDER.to_owned()));
    };
    let caps = match cb.caps(&ProviderId::new(provider)) {
        Ok(caps) => caps,
        Err(DriverError::Unsupported(_)) => {
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
            run(cb, "pause", handle, Op::Sleep)?;
            Ok(SleepOutcome::Slept)
        }
    }
}

/// Wakes the Box as its provider's facts allow; a native sleeper needs no call.
pub(super) fn wake(cb: &Crabbox, handle: &BoxHandle) -> Result<(), DriverError> {
    let Some(provider) = split_handle(handle).0 else {
        return Err(DriverError::Unsupported(
            "box has no known provider to wake",
        ));
    };
    match cb.caps(&ProviderId::new(provider))?.sleep {
        SleepKind::Native => Ok(()),
        SleepKind::Suspend => run(cb, "resume", handle, Op::Wake),
        SleepKind::None => Err(DriverError::Unsupported("provider cannot wake a box")),
    }
}

/// Runs `crabbox <verb>` for the Box.
fn run(cb: &Crabbox, verb: &str, handle: &BoxHandle, op: Op) -> Result<(), DriverError> {
    let spec = Crabbox::spec(PAUSE_DEADLINE)
        .arg(verb)
        .args(target_args(handle));
    let spec = cb.at_lease(spec, handle)?;
    cb.call(op, split_handle(handle).0, &spec).map(|_| ())
}

#[cfg(test)]
mod tests;
