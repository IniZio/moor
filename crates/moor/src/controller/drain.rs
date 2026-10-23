//! Draining on SIGTERM: stop accepting, let what is in flight finish, then return.
//!
//! A connect that arrives once the drain has begun is not accepted. It waits in the listen
//! backlog and fails when the controller exits and the listener closes. One accepted in the
//! instant the drain began gets a `restarting` frame.

use std::io;
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use rustix::event::{poll, PollFd, PollFlags};
use rustix::io::Errno;
use signal_hook::consts::SIGTERM;

use super::fronts::Terminator;
use super::{conn, log, Sink};
use crate::Moor;

/// The connections one controller serves and the settle pass it runs, and whether it has
/// begun to drain.
#[derive(Debug, Default)]
pub(crate) struct Drain {
    /// Set once the drain begins; never cleared.
    begun: AtomicBool,
    /// Each connection being served and each settle pass running.
    live: Mutex<Live>,
    /// Notified each time one ends.
    ended: Condvar,
    /// Stops the fronts when the drain begins.
    fronts: Mutex<Option<Terminator>>,
}

/// The work the drain waits for: each connection with a handle that ends its reads while it
/// is idle, and each settle pass with none.
#[derive(Debug, Default)]
struct Live {
    /// Number for the next one admitted.
    next: u64,
    /// Admitted work by number; a connection's handle is out while it answers a request, so
    /// the drain leaves its reads alone.
    streams: Vec<(u64, Option<UnixStream>)>,
}

impl Drain {
    /// Whether the drain has begun.
    pub(crate) fn begun(&self) -> bool {
        self.begun.load(Ordering::Acquire)
    }

    /// Has `fronts` end with the drain; they get SIGTERM the moment it begins.
    pub(crate) fn end_fronts_with_drain(&self, fronts: Terminator) {
        *self.fronts.lock().unwrap_or_else(PoisonError::into_inner) = Some(fronts);
    }

    /// Admits `stream` for serving, or `None` once the drain has begun.
    fn admit(self: &Arc<Self>, stream: &UnixStream) -> io::Result<Option<Admitted>> {
        Ok(self.enter(Some(stream.try_clone()?)))
    }

    /// Admits one settle pass, or `None` once the drain has begun; the drain waits for it.
    pub(crate) fn hold(self: &Arc<Self>) -> Option<Admitted> {
        self.enter(None)
    }

    /// Admits work the drain waits for, or `None` once the drain has begun.
    fn enter(self: &Arc<Self>, stream: Option<UnixStream>) -> Option<Admitted> {
        let mut live = self.live();
        if self.begun() {
            return None;
        }
        let id = live.next;
        live.next += 1;
        live.streams.push((id, stream));
        drop(live);
        Some(Admitted {
            drain: Arc::clone(self),
            id,
        })
    }

    /// Begins the drain. A connection waiting for its next request reads EOF; one in the
    /// middle of a request keeps reading, so its client still counts as present, until its
    /// answer is ready.
    pub(crate) fn begin(&self) {
        let live = self.live();
        self.begun.store(true, Ordering::Release);
        for stream in live.streams.iter().filter_map(|(_, s)| s.as_ref()) {
            stream.shutdown(Shutdown::Read).ok();
        }
        drop(live);
        if let Some(fronts) = &*self.fronts.lock().unwrap_or_else(PoisonError::into_inner) {
            fronts.terminate();
        }
    }

    /// Waits up to `timeout` for all admitted work to end; returns how much remains.
    fn settle(&self, timeout: Duration) -> usize {
        self.ended
            .wait_timeout_while(self.live(), timeout, |l| !l.streams.is_empty())
            .unwrap_or_else(PoisonError::into_inner)
            .0
            .streams
            .len()
    }

    /// The live set, locked.
    fn live(&self) -> MutexGuard<'_, Live> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One admitted connection or settle pass; its end wakes [`Drain::settle`].
pub(crate) struct Admitted {
    /// The drain that admitted it.
    drain: Arc<Drain>,
    /// Its number in the live set.
    id: u64,
}

impl Admitted {
    /// The drain that admitted this connection.
    pub(crate) const fn drain(&self) -> &Arc<Drain> {
        &self.drain
    }

    /// Marks the connection as answering a request until the guard drops, or `None` once the
    /// drain has begun: a request read after that does not run.
    pub(crate) fn answering(&self) -> Option<Answering<'_>> {
        let mut live = self.drain.live();
        if self.drain.begun() {
            return None;
        }
        let handle = live
            .streams
            .iter_mut()
            .find(|(id, _)| *id == self.id)
            .and_then(|(_, s)| s.take());
        drop(live);
        Some(Answering { of: self, handle })
    }

    /// A connection no drain waits for, for tests that serve one by hand.
    #[cfg(test)]
    pub(crate) fn unwatched() -> Self {
        Self {
            drain: Arc::new(Drain::default()),
            id: u64::MAX,
        }
    }
}

/// A request one connection is answering; the drain leaves the connection's reads alone
/// until it drops, and ends them then if it began meanwhile.
pub(crate) struct Answering<'a> {
    /// The connection answering it.
    of: &'a Admitted,
    /// The connection's read handle, held back from the drain.
    handle: Option<UnixStream>,
}

impl Drop for Answering<'_> {
    fn drop(&mut self) {
        let mut live = self.of.drain.live();
        if self.of.drain.begun() {
            if let Some(handle) = &self.handle {
                handle.shutdown(Shutdown::Read).ok();
            }
        }
        if let Some(slot) = live.streams.iter_mut().find(|(id, _)| *id == self.of.id) {
            slot.1 = self.handle.take();
        }
    }
}

impl Drop for Admitted {
    fn drop(&mut self) {
        self.drain.live().streams.retain(|(id, _)| *id != self.id);
        self.drain.ended.notify_all();
    }
}

/// Serves one accepted connection to its end.
pub(super) type Handler = Arc<dyn Fn(UnixStream, Admitted) + Send + Sync>;

/// Takes SIGTERM for the controller: the returned socket turns readable once it arrives.
/// From here on a streamed command never sees SIGINT or SIGTERM.
pub(super) fn on_sigterm() -> io::Result<UnixStream> {
    if !Moor::leave_signals_to_controller() {
        return Err(io::Error::other(
            "a streamed command already took the signals",
        ));
    }
    let (wake, signalled) = UnixStream::pair()?;
    signal_hook::low_level::pipe::register(SIGTERM, signalled)?;
    Ok(wake)
}

/// Serves every listener until `wake` turns readable, then drains: nothing more is accepted,
/// and the admitted work gets up to `timeout` to end. Returns how much was cut off.
pub(super) fn serve_until(
    listeners: &[(UnixListener, Handler)],
    wake: &UnixStream,
    drain: &Arc<Drain>,
    timeout: Duration,
    sink: &Sink,
) -> usize {
    thread::scope(|scope| {
        for (listener, handler) in listeners {
            scope.spawn(move || accept(listener, handler, wake, drain, sink));
        }
        readable(wake);
        drain.begin();
        log(sink, "draining: no new connections");
    });
    drain.settle(timeout)
}

/// Accepts on `listener` until `wake` turns readable, serving each connection on its own thread.
fn accept(
    listener: &UnixListener,
    handler: &Handler,
    wake: &UnixStream,
    drain: &Arc<Drain>,
    sink: &Sink,
) {
    loop {
        let mut fds = [
            PollFd::new(wake, PollFlags::IN),
            PollFd::new(listener, PollFlags::IN),
        ];
        match poll(&mut fds, None) {
            Ok(_) => {}
            Err(Errno::INTR) => continue,
            Err(e) => {
                log(sink, &format!("cannot wait for connections: {e}"));
                return;
            }
        }
        let [woken, ready] = &fds;
        if !woken.revents().is_empty() {
            return;
        }
        if ready.revents().is_empty() {
            continue;
        }
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(e) => {
                log(sink, &format!("accept failed: {e}"));
                continue;
            }
        };
        let admitted = match drain.admit(&stream) {
            Ok(Some(admitted)) => admitted,
            Ok(None) => {
                conn::send(&mut &stream, &conn::restarting());
                return;
            }
            Err(e) => {
                log(sink, &format!("connection setup failed: {e}"));
                continue;
            }
        };
        let handler = Arc::clone(handler);
        thread::spawn(move || handler(stream, admitted));
    }
}

/// Blocks until `wake` turns readable.
fn readable(wake: &UnixStream) {
    loop {
        let mut fds = [PollFd::new(wake, PollFlags::IN)];
        match poll(&mut fds, None) {
            Err(Errno::INTR) => {}
            Ok(_) | Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests;
