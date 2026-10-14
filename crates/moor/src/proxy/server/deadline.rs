//! A reader that bounds the total time spent reading from a connection.

use std::io::{self, ErrorKind, Read};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use moor_core::seam::clock::Clock;

/// Stand-in budget when the requested one overflows the clock: one year.
const FAR_FUTURE: Duration = Duration::from_hours(365 * 24);

/// Reads from a TCP stream until a fixed point in time, however many reads
/// that takes. A per-read timeout alone lets a peer that sends one byte just
/// before each timeout keep the connection open indefinitely.
pub(crate) struct DeadlineReader<'a> {
    /// The connection being read.
    stream: &'a TcpStream,
    /// Source of the current time.
    clock: &'a dyn Clock,
    /// Reads fail with `TimedOut` once the clock reaches this instant.
    deadline: Instant,
}

impl<'a> DeadlineReader<'a> {
    /// Starts a reader whose deadline is `budget` from now.
    pub(crate) fn new(stream: &'a TcpStream, clock: &'a dyn Clock, budget: Duration) -> Self {
        let now = clock.instant();
        let deadline = now
            .checked_add(budget)
            .or_else(|| now.checked_add(FAR_FUTURE))
            .unwrap_or(now);
        Self {
            stream,
            clock,
            deadline,
        }
    }
}

impl Read for DeadlineReader<'_> {
    /// Reads once, waiting no longer than the time left before the deadline.
    ///
    /// A timeout always surfaces as [`ErrorKind::TimedOut`], whichever kind
    /// the platform reports for an expired socket timeout.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let remaining = self
            .deadline
            .saturating_duration_since(self.clock.instant());
        if remaining.is_zero() {
            return Err(io::Error::from(ErrorKind::TimedOut));
        }
        self.stream.set_read_timeout(Some(remaining))?;
        (&*self.stream).read(buf).map_err(|e| {
            if e.kind() == ErrorKind::WouldBlock {
                io::Error::from(ErrorKind::TimedOut)
            } else {
                e
            }
        })
    }
}

#[cfg(test)]
mod tests;
