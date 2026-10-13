//! Reader that fails once a body passes its byte cap.

use std::error::Error;
use std::fmt;
use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Marker carried inside the io error returned at the cap.
#[derive(Debug)]
struct CapExceeded;

impl fmt::Display for CapExceeded {
    /// Fixed message, so callers can log it without the byte counts.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("request body exceeds the route cap")
    }
}

impl Error for CapExceeded {}

/// Passes through at most `cap` bytes of `inner`; reading past the cap fails.
pub(super) struct CappedReader<R: Read> {
    /// Source of the body.
    inner: R,
    /// Most bytes that may be returned.
    cap: u64,
    /// Bytes read from `inner` so far.
    count: u64,
    /// Set once the cap is passed; the error is then returned on every read.
    over: bool,
    /// Shared copy of `over`, readable after the HTTP client wraps the error.
    tripped: Arc<AtomicBool>,
}

impl<R: Read> CappedReader<R> {
    /// Wraps `inner`, setting `tripped` when it yields more than `cap` bytes.
    pub(super) const fn new(inner: R, cap: u64, tripped: Arc<AtomicBool>) -> Self {
        Self {
            inner,
            cap,
            count: 0,
            over: false,
            tripped,
        }
    }

    /// Records the overflow and builds the error for it.
    fn trip(&mut self) -> io::Error {
        self.over = true;
        self.tripped.store(true, Ordering::SeqCst);
        io::Error::new(io::ErrorKind::InvalidData, CapExceeded)
    }
}

impl<R: Read> Read for CappedReader<R> {
    /// Reads at most one byte past the cap so overflow is seen but never returned.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.over {
            return Err(self.trip());
        }
        if buf.is_empty() {
            return Ok(0);
        }
        let room = self.cap.saturating_sub(self.count).saturating_add(1);
        let limit = usize::try_from(room).map_or(buf.len(), |r| r.min(buf.len()));
        let Some(window) = buf.get_mut(..limit) else {
            return Ok(0);
        };
        let n = self.inner.read(window)?;
        self.count = self
            .count
            .saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
        if self.count > self.cap {
            return Err(self.trip());
        }
        Ok(n)
    }
}

/// True when `err` is the error `CappedReader` returns at the cap.
pub(super) fn is_cap_exceeded(err: &io::Error) -> bool {
    err.get_ref()
        .is_some_and(<dyn Error + Send + Sync>::is::<CapExceeded>)
}

#[cfg(test)]
mod tests;
