//! The connections that stay open on purpose, which a drain closes after a short bound
//! instead of waiting on.

use std::net::{Shutdown, TcpStream};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// The streams held open, and whether the drain has closed them.
#[derive(Default)]
struct Open {
    /// Set once the drain gave up waiting; later streams are closed as they arrive.
    closing: bool,
    /// Number for the next stream held.
    next: u64,
    /// A handle on each stream held, by number.
    held: Vec<(u64, TcpStream)>,
    /// Requests waiting for the controller.
    waiting: usize,
    /// `/mcp` requests whose response has not begun.
    unanswered: usize,
}

/// Every long-lived connection the gate serves.
#[derive(Default)]
pub(crate) struct Streams(Mutex<Open>, Condvar);

/// A stream counted as long-lived until this drops.
pub(crate) struct Hold<'a> {
    /// Where it is counted.
    streams: &'a Streams,
    /// Its number.
    id: u64,
}

/// An `/mcp` request counted as unanswered until this drops.
pub(crate) struct Unanswered<'a>(&'a Streams);

/// A request counted as waiting for the controller until this drops.
pub(crate) struct Waiting<'a>(&'a Streams);

impl Streams {
    /// The state, usable even when a holder panicked.
    fn open(&self) -> MutexGuard<'_, Open> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Counts `stream` as long-lived; the drain closes it after its bound.
    pub(crate) fn hold(&self, stream: &TcpStream) -> Option<Hold<'_>> {
        let handle = stream.try_clone().ok()?;
        let mut open = self.open();
        if open.closing {
            let _ = handle.shutdown(Shutdown::Both);
        }
        open.next += 1;
        let id = open.next;
        open.held.push((id, handle));
        drop(open);
        Some(Hold { streams: self, id })
    }

    /// Counts an `/mcp` request as unanswered. Ending the MCP streams fails such a request with
    /// a 500, so the drain waits for it first.
    pub(crate) fn unanswered(&self) -> Unanswered<'_> {
        self.open().unanswered += 1;
        Unanswered(self)
    }

    /// Waits up to `bound` for every `/mcp` request to have its response begun.
    pub(crate) fn wait_for_answers(&self, bound: Duration) {
        drop(
            self.1
                .wait_timeout_while(self.open(), bound, |open| open.unanswered > 0),
        );
    }

    /// Counts a request as waiting for the controller, unless [`crate::HOLD_CAP`] already are.
    pub(crate) fn wait(&self) -> Option<Waiting<'_>> {
        let mut open = self.open();
        if open.waiting >= crate::HOLD_CAP {
            return None;
        }
        open.waiting += 1;
        drop(open);
        Some(Waiting(self))
    }

    /// Closes every stream held now and every one held later.
    pub(crate) fn close_all(&self) {
        let mut open = self.open();
        open.closing = true;
        for (_, stream) in &open.held {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

impl Drop for Hold<'_> {
    fn drop(&mut self) {
        self.streams.open().held.retain(|(id, _)| *id != self.id);
    }
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.open().waiting -= 1;
    }
}

impl Drop for Unanswered<'_> {
    fn drop(&mut self) {
        self.0.open().unanswered -= 1;
        self.0 .1.notify_all();
    }
}
