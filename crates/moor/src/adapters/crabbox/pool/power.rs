//! Sleeping and waking a pool Box's VM.

use moor_client::link::lease::{Answer, Verb};
use moor_core::name::BoxHandle;
use moor_core::seam::driver::{DriverError, PausePublish, SleepOutcome};

use super::{split_handle, unexpected, Crabbox};

/// Pauses the VM, or says why it cannot.
pub(in crate::adapters::crabbox) fn sleep(
    cb: &Crabbox,
    handle: &BoxHandle,
    publish: PausePublish,
) -> Result<SleepOutcome, DriverError> {
    let verb = match publish {
        PausePublish::Allowed => Verb::PausePublish,
        PausePublish::Denied => Verb::Pause,
    };
    match cb.placed(handle)?.ask(verb, None)? {
        Answer::Done => Ok(SleepOutcome::Slept),
        Answer::Unsupported => Ok(SleepOutcome::CannotSleep(format!(
            "provider {} cannot sleep a box",
            split_handle(handle).0.unwrap_or_default()
        ))),
        other => Err(unexpected(verb, &other)),
    }
}

/// Resumes the VM.
pub(in crate::adapters::crabbox) fn wake(
    cb: &Crabbox,
    handle: &BoxHandle,
) -> Result<(), DriverError> {
    match cb.placed(handle)?.ask(Verb::Resume, None)? {
        Answer::Done => Ok(()),
        Answer::Unsupported => Err(DriverError::Unsupported("provider cannot wake a box")),
        other => Err(unexpected(Verb::Resume, &other)),
    }
}
