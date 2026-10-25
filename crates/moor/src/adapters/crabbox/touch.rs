//! Keeps a Box's lease alive while the Box is really used.
//!
//! crabbox expires a lease that nothing touched for its idle timeout. Every `crabbox ssh`
//! lookup touches the lease, so moor reaches a Box through its saved target and touches the
//! lease only with `crabbox heartbeat`, on real use: a start, a steer, a command or run, a file
//! copy, a wake, or a turn the agent's hooks report busy. A Box that only gets looked at
//! reaches crabbox's idle expiry.

use moor_core::name::BoxHandle;
use moor_core::seam::driver::DriverError;

use super::errors::Op;
use super::{split_handle, target_args, Crabbox};

/// Milliseconds within which real use of a Box sends no second heartbeat; far inside crabbox's
/// 30 min default idle timeout.
const TOUCH_GAP_MS: i64 = 5 * 60 * 1000;

/// Touches the lease of the Box behind `handle`, unless this driver touched it in the last
/// [`TOUCH_GAP_MS`].
pub(super) fn heartbeat(cb: &Crabbox, handle: &BoxHandle) -> Result<(), DriverError> {
    let now = cb.clock.now().as_millis();
    let last = cb
        .touched
        .lock()
        .ok()
        .and_then(|m| m.get(handle.as_str()).copied());
    if last.is_some_and(|at| (0..TOUCH_GAP_MS).contains(&now.saturating_sub(at.as_millis()))) {
        return Ok(());
    }
    send(cb, handle)
}

/// Sends `crabbox heartbeat` for the Box behind `handle`, from its lease dir.
pub(super) fn send(cb: &Crabbox, handle: &BoxHandle) -> Result<(), DriverError> {
    let provider = split_handle(handle).0;
    let spec = Crabbox::spec(cb.lookup_deadline(provider.unwrap_or_default()))
        .arg("heartbeat")
        .args(target_args(handle));
    let spec = cb.at_lease(spec, handle)?;
    cb.call(Op::Touch, provider, &spec)?;
    if let Ok(mut m) = cb.touched.lock() {
        m.insert(handle.as_str().to_owned(), cb.clock.now());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
