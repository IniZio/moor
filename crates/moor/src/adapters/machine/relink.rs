//! Calls safe to repeat, asked again while the machine's link is down: a runner that restarts or
//! a link that drops and redials leaves the machine out of reach for a few seconds, which is not
//! the call failing.

use std::time::Duration;

use moor_client::link::boxes::{BoxAnswer, BoxOp};
use moor_client::link::broker;
use moor_core::seam::driver::{DriverError, LINK_WAIT};

use super::stream::Timer;
use super::Machines;

/// Pause before asking again.
const POLL: Duration = Duration::from_secs(1);
/// A call that ran this long reached the machine, so a loss after it starts a new wait.
const REACHED: Duration = Duration::from_secs(5);

impl Machines {
    /// [`Self::call`] for a verb safe to send twice, sent again while the machine's link is down.
    pub(super) fn call_again(
        &self,
        machine: i64,
        op: &BoxOp,
        note: &mut (dyn FnMut(&str) + Send),
    ) -> Result<BoxAnswer, DriverError> {
        relinked(|| self.call(machine, op, &mut *note))
    }
}

/// Whether `e` is the machine's link down or cut, so the call may not have reached it.
pub(crate) fn link_down(e: &DriverError) -> bool {
    match e {
        DriverError::Unavailable(why) | DriverError::Transport { stderr_tail: why } => {
            broker::is_link_loss(why)
        }
        _ => false,
    }
}

/// Runs `call`, and again while it fails with the link down, until the link has stayed down for
/// [`LINK_WAIT`]; then its last failure. Only for calls safe to run twice.
pub(super) fn relinked<T>(call: impl FnMut() -> Result<T, DriverError>) -> Result<T, DriverError> {
    relinked_within((LINK_WAIT, POLL, REACHED), call)
}

/// [`relinked`] waiting `wait`, asking again after `poll`, a call that ran `reached` having
/// reached the machine.
fn relinked_within<T>(
    (wait, poll, reached): (Duration, Duration, Duration),
    mut call: impl FnMut() -> Result<T, DriverError>,
) -> Result<T, DriverError> {
    let mut gap: Option<Timer> = None;
    loop {
        let ran_long = Timer::after(reached);
        let got = call();
        match &got {
            Err(e) if link_down(e) => {}
            _ => return got,
        }
        if ran_long.is_up() {
            gap = None;
        }
        if gap.get_or_insert_with(|| Timer::after(wait)).is_up() {
            return got;
        }
        std::thread::sleep(poll);
    }
}

#[cfg(test)]
mod tests;
