//! Tells when the client of a connection is gone: it closed the socket or shut its sending side.

use std::io;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use rustix::event::{poll, PollFd, PollFlags, Timespec};
use rustix::net::{recv, RecvFlags};

use crate::env::Cancel;

/// Longest wait for a hangup before the watch checks whether the work is done.
const TICK: Timespec = Timespec {
    tv_sec: 0,
    tv_nsec: 50_000_000,
};

/// Pause while the client has sent bytes nobody has read yet, which hide a hangup behind them.
const PENDING: Duration = Duration::from_millis(50);

/// Watches one connection for its client going away.
#[derive(Debug)]
pub(crate) struct Hangup(UnixStream);

impl Hangup {
    /// A watch on the connection of `stream`.
    ///
    /// # Errors
    /// The socket cannot be shared.
    pub(crate) fn of(stream: &UnixStream) -> io::Result<Self> {
        stream.try_clone().map(Self)
    }

    /// Runs `work`, setting `cancel` as soon as the client hangs up while it runs.
    pub(crate) fn cancels<T>(&self, cancel: &Cancel, work: impl FnOnce() -> T) -> T {
        let done = AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| {
                while !done.load(Ordering::SeqCst) {
                    if self.gone() {
                        cancel.cancel();
                        return;
                    }
                }
            });
            let out = work();
            done.store(true, Ordering::SeqCst);
            out
        })
    }

    /// Waits up to [`TICK`] and says whether the client has gone.
    fn gone(&self) -> bool {
        let mut fds = [PollFd::new(&self.0, PollFlags::IN)];
        if !matches!(poll(&mut fds, Some(&TICK)), Ok(n) if n > 0) {
            return false;
        }
        let events = fds.first().map_or(PollFlags::empty(), PollFd::revents);
        if events.intersects(PollFlags::HUP | PollFlags::ERR) {
            return true;
        }
        let mut byte = [0u8; 1];
        match recv(&self.0, &mut byte, RecvFlags::PEEK | RecvFlags::DONTWAIT) {
            Ok((0, _)) => true,
            Ok(_) => {
                thread::sleep(PENDING);
                false
            }
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests;
