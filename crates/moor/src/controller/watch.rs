//! `watch` streaming: replays stored events after a cursor, then follows new ones.

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use moor_core::controller::{FrameErrorKind, ServerFrame, WatchFilter};
use moor_core::user::Viewer;
use moor_core::{BoxId, SessionId};

use super::drain::Drain;
use super::hangup::Hangup;
use crate::env::{Cancel, Env};
use crate::Moor;

/// Sizes and pacing of one watch stream.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    /// Frames queued for the client before it counts as slow.
    pub(crate) buffer: usize,
    /// Pause between store reads once the stream has caught up.
    pub(crate) poll: Duration,
    /// Most events read from the store at once.
    pub(crate) batch: usize,
    /// Longest a following stream stays silent: then it sends a `caught-up` frame, so a client
    /// that vanished without closing its socket fails the write and ends the stream.
    pub(crate) keepalive: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            buffer: 1024,
            poll: Duration::from_millis(200),
            batch: 256,
            keepalive: Duration::from_secs(5),
        }
    }
}

/// Flags shared by the connection thread and its poller.
#[derive(Debug, Default)]
struct Shared {
    /// The poller stopped because the buffer was full.
    overflow: AtomicBool,
    /// The connection is finished; the poller must stop.
    closed: AtomicBool,
    /// The poller reached the end of the store with follow off.
    caught_up: AtomicBool,
    /// Last event id the poller scanned, including events the filter skipped.
    scanned: AtomicI64,
    /// Why the poller could not open or read the store, if that is why it stopped.
    failure: Mutex<Option<String>>,
    /// The controller's drain; the poller stops once it begins.
    drain: Option<Arc<Drain>>,
    /// The poller stopped because the drain began.
    restarting: AtomicBool,
    /// Set once the client hangs up; the poller stops then.
    gone: Cancel,
}

impl Shared {
    /// Record why the store failed.
    fn fail(&self, reason: String) {
        if let Ok(mut slot) = self.failure.lock() {
            *slot = Some(reason);
        }
    }

    /// Whether the poller must stop: the connection closed, the client hung up, or the drain
    /// began, which the stream then reports.
    fn stopping(&self) -> bool {
        if self.drain.as_ref().is_some_and(|d| d.begun()) {
            self.restarting.store(true, Ordering::Release);
            return true;
        }
        self.closed.load(Ordering::Acquire) || self.gone.is_cancelled()
    }

    /// Take the recorded store failure.
    fn take_failure(&self) -> Option<String> {
        self.failure.lock().ok()?.take()
    }
}

/// Watch streams whose poller runs, across the process.
static LIVE: AtomicUsize = AtomicUsize::new(0);

/// How many watch streams are open now.
pub(crate) fn live() -> usize {
    LIVE.load(Ordering::Acquire)
}

/// Counts one poller in [`LIVE`] until it ends, however it ends.
struct Live;

impl Live {
    /// Counts a poller that starts now.
    fn start() -> Self {
        LIVE.fetch_add(1, Ordering::AcqRel);
        Self
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        LIVE.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Marks the stream closed when the connection thread leaves, however it leaves.
struct CloseOnDrop(Arc<Shared>);

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        self.0.closed.store(true, Ordering::Release);
    }
}

/// What a client asked to watch.
#[derive(Debug, Clone)]
pub(crate) struct Ask {
    /// Only events with a cursor above this.
    pub(crate) since: i64,
    /// Only events this matches.
    pub(crate) filter: WatchFilter,
    /// Keep streaming new events after catching up.
    pub(crate) follow: bool,
    /// Whose Boxes' events the stream carries.
    pub(crate) viewer: Viewer,
}

/// Stream every stored event with a cursor above `since` that `filter` matches to `writer`, then follow new ones.
///
/// With `follow` off the stream sends a `caught-up` frame and ends once it has caught up. Otherwise it returns when the writer fails, the store cannot be read, or the client falls
/// behind by more than `limits.buffer` frames. A following stream that sends nothing for
/// `limits.keepalive` sends a `caught-up` frame with the cursor scanned so far. A slow client gets a `slow-client`
/// error frame and a store failure a `store-unavailable` one. The client resumes from the last cursor it received.
pub(crate) fn stream(writer: &mut impl Write, env: &Env, ask: Ask, limits: Limits) {
    run(writer, env, ask, limits, &Arc::new(Shared::default()));
}

/// [`stream`] that ends with a `restarting` error frame once `drain` begins, and ends without a
/// word once `gone` is set.
pub(crate) fn stream_draining(
    writer: &mut impl Write,
    env: &Env,
    ask: Ask,
    limits: Limits,
    (drain, gone): (&Arc<Drain>, Cancel),
) {
    let shared = Shared {
        drain: Some(Arc::clone(drain)),
        gone,
        ..Shared::default()
    };
    run(writer, env, ask, limits, &Arc::new(shared));
}

/// [`stream_draining`] that also ends once `hangup` sees the client leave. Nothing reads the
/// connection while it streams, so a follow with nothing to send would not notice otherwise.
pub(crate) fn stream_on(
    writer: &mut impl Write,
    env: &Env,
    ask: Ask,
    drain: &Arc<Drain>,
    hangup: Option<&Hangup>,
) {
    let gone = Cancel::default();
    let stream = || stream_draining(writer, env, ask, Limits::default(), (drain, gone.clone()));
    match hangup {
        Some(hangup) => hangup.cancels(&gone, stream),
        None => stream(),
    }
}

/// [`stream`] with the shared flags supplied by the caller.
fn run(writer: &mut impl Write, env: &Env, ask: Ask, limits: Limits, shared: &Arc<Shared>) {
    let _close = CloseOnDrop(Arc::clone(shared));
    let (tx, rx) = sync_channel(limits.buffer);
    let env = env.clone();
    let poller = Arc::clone(shared);
    let (since, follow) = (ask.since, ask.follow);
    thread::spawn(move || crate::Moor::checked(|| poll(&env, &ask, limits, &tx, &poller)));
    let mut last = since;
    loop {
        let frame = match rx.recv_timeout(limits.keepalive) {
            Ok(frame) => frame,
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                // Read before looking at the queue: the poller queues an event before it moves
                // `scanned` past it, so an empty queue now means every event up to `cursor` went.
                let cursor = shared.scanned.load(Ordering::Acquire).max(last);
                match rx.try_recv() {
                    Ok(frame) => frame,
                    Err(TryRecvError::Disconnected) => break,
                    Err(TryRecvError::Empty) if follow => ServerFrame::CaughtUp { cursor },
                    Err(TryRecvError::Empty) => continue,
                }
            }
        };
        if let ServerFrame::Event { cursor, .. } = &frame {
            last = *cursor;
        }
        if send(writer, &frame).is_err() {
            return;
        }
    }
    // The channel only ends after the poller stopped, so the flag is final here.
    if shared.overflow.load(Ordering::Acquire) {
        let slow = ServerFrame::Error {
            id: None,
            kind: FrameErrorKind::SlowClient,
            message: format!("client too slow; last delivered cursor {last}, reconnect with it"),
        };
        let _ = send(writer, &slow);
    } else if let Some(reason) = shared.take_failure() {
        let unavailable = ServerFrame::Error {
            id: None,
            kind: FrameErrorKind::StoreUnavailable,
            message: format!("event store unavailable ({reason}); last delivered cursor {last}, reconnect with it"),
        };
        let _ = send(writer, &unavailable);
    } else if shared.restarting.load(Ordering::Acquire) {
        let restarting = ServerFrame::Error {
            id: None,
            kind: FrameErrorKind::Restarting,
            message: format!(
                "the controller is restarting; last delivered cursor {last}, reconnect with it"
            ),
        };
        let _ = send(writer, &restarting);
    } else if shared.caught_up.load(Ordering::Acquire) {
        let cursor = shared.scanned.load(Ordering::Acquire).max(last);
        let _ = send(writer, &ServerFrame::CaughtUp { cursor });
    }
}

/// Write one frame and flush it.
fn send(writer: &mut impl Write, frame: &ServerFrame) -> std::io::Result<()> {
    writer.write_all(frame.to_line().as_bytes())?;
    writer.flush()
}

/// Read events after `since`, queueing matches without ever waiting on the client.
///
/// The cursor advances for filtered-out events and for queued ones, so a full buffer
/// leaves no gap. Without `follow` it stops after a short read. A store failure is
/// recorded in `shared` for the connection to report.
fn poll(env: &Env, ask: &Ask, limits: Limits, tx: &SyncSender<ServerFrame>, shared: &Shared) {
    let _live = Live::start();
    // `Moor` is not `Send`, so it is built here rather than handed to the thread.
    let moor = match Moor::from_env(env) {
        Ok(moor) => moor.viewing_as(ask.viewer),
        Err(e) => return shared.fail(e.error.to_string()),
    };
    let mut passes = Passes {
        moor: &moor,
        ask,
        seen: HashMap::new(),
        parents: HashMap::new(),
    };
    let follow = ask.follow;
    let mut cursor = ask.since;
    shared.scanned.store(cursor, Ordering::Release);
    while !shared.stopping() {
        let batch = match moor.events_after(cursor, limits.batch) {
            Ok(batch) => batch,
            Err(reason) => return shared.fail(reason),
        };
        if batch.is_empty() {
            if !follow {
                shared.caught_up.store(true, Ordering::Release);
                return;
            }
            thread::sleep(limits.poll);
            continue;
        }
        let short = batch.len() < limits.batch;
        for frame in batch {
            let id = match &frame {
                ServerFrame::Event { cursor, .. } => *cursor,
                _ => cursor,
            };
            match passes.check(&frame) {
                Ok(true) => {}
                Ok(false) => {
                    cursor = id;
                    shared.scanned.store(cursor, Ordering::Release);
                    continue;
                }
                Err(reason) => return shared.fail(reason),
            }
            match tx.try_send(frame) {
                Ok(()) => {
                    cursor = id;
                    shared.scanned.store(cursor, Ordering::Release);
                }
                Err(TrySendError::Full(_)) => {
                    shared.overflow.store(true, Ordering::Release);
                    return;
                }
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
        if short && !follow {
            shared.caught_up.store(true, Ordering::Release);
            return;
        }
    }
}

/// Which stored events one stream sends: those its filter matches, of Boxes its viewer may
/// see. An event with no Box is the operator's alone.
struct Passes<'a> {
    /// The engine seeing as the stream's viewer.
    moor: &'a Moor,
    /// What the client asked for.
    ask: &'a Ask,
    /// Whether the viewer may see each Box met so far; a Box's user never changes.
    seen: HashMap<BoxId, bool>,
    /// The parent of each Session met so far; a Session's parent never changes.
    parents: HashMap<SessionId, Option<SessionId>>,
}

impl Passes<'_> {
    /// Whether `frame` goes to the client.
    fn check(&mut self, frame: &ServerFrame) -> Result<bool, String> {
        let ServerFrame::Event {
            r#box,
            box_id,
            session,
            kind,
            ..
        } = frame
        else {
            return Ok(true);
        };
        if !self.ask.filter.matches(r#box.as_deref(), kind) {
            return Ok(false);
        }
        if let Some(parent) = self.ask.filter.parent {
            if self.parent_of(*session)? != Some(parent) {
                return Ok(false);
            }
        }
        let Some(id) = *box_id else {
            return Ok(self.ask.viewer.may_act_on(None));
        };
        if let Some(&visible) = self.seen.get(&id) {
            return Ok(visible);
        }
        let visible = self.moor.may_see(id)?;
        self.seen.insert(id, visible);
        Ok(visible)
    }

    /// The parent of `session`; none for an event with no Session.
    fn parent_of(&mut self, session: Option<SessionId>) -> Result<Option<SessionId>, String> {
        let Some(id) = session else { return Ok(None) };
        if let Some(&parent) = self.parents.get(&id) {
            return Ok(parent);
        }
        let parent = self.moor.session_parent(id)?;
        self.parents.insert(id, parent);
        Ok(parent)
    }
}

#[cfg(test)]
mod tests;
