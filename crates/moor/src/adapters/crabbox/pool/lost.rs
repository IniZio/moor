//! Telling a host out of reach from a host that said no.

use moor_client::link::broker::is_link_loss;
use moor_core::name::BoxHandle;
use moor_core::seam::driver::DriverError;

use super::{failed, release};
use crate::adapters::crabbox::Crabbox;

/// `e`, which came after the VM booted, once the lease is stopped. A lost link leaves the VM to
/// whoever takes the acquire over.
pub(in crate::adapters::crabbox) fn undone(
    cb: &Crabbox,
    handle: &BoxHandle,
    e: DriverError,
) -> DriverError {
    if !matches!(e, DriverError::Transport { .. }) {
        let _ = release(cb, handle);
    }
    e
}

/// The failure of a call to the host: a lost link when that is what `why` says.
pub(in crate::adapters::crabbox) fn lost_or_failed(why: String) -> DriverError {
    if is_link_loss(&why) {
        return DriverError::Transport { stderr_tail: why };
    }
    failed(why)
}
