//! The connections that stay open on purpose, which a drain closes after a short bound
//! instead of waiting on.

use std::net::{Shutdown, TcpStream};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// The streams held open, and whether the drain has closed them.
#[derive(Default)]
struct Open {
    /// Set once the drain gave up waiting; later streams are closed as they arrive.
    closing: bool,
    /// Number for the next stream held.
    next: u64,
    /// A handle on each stream held, by number.
    held: Vec<(u64, TcpStream)>,
}

/// Every long-lived connection the gate serves.
#[derive(Default)]
pub(crate) struct Streams(Mutex<Open>);

/// A stream counted as long-lived until this drops.
pub(crate) struct Hold<'a> {
    /// Where it is counted.
    streams: &'a Streams,
    /// Its number.
    id: u64,
}

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
