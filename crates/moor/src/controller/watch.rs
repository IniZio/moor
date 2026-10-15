//! `watch` streaming: replays stored events after a cursor, then follows new ones.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use moor_core::controller::{FrameErrorKind, ServerFrame};

use crate::env::Env;
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
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            buffer: 1024,
            poll: Duration::from_millis(200),
            batch: 256,
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
    /// Why the poller could not open or read the store, if that is why it stopped.
    failure: Mutex<Option<String>>,
}

impl Shared {
    /// Record why the store failed.
    fn fail(&self, reason: String) {
        if let Ok(mut slot) = self.failure.lock() {
            *slot = Some(reason);
        }
    }

    /// Take the recorded store failure.
    fn take_failure(&self) -> Option<String> {
        self.failure.lock().ok()?.take()
    }
}

/// Marks the stream closed when the connection thread leaves, however it leaves.
struct CloseOnDrop(Arc<Shared>);

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        self.0.closed.store(true, Ordering::Release);
    }
}

/// Stream every stored event with a cursor above `since` to `writer`, then follow new ones.
///
/// Returns when the writer fails, the store cannot be read, or the client falls
/// behind by more than `limits.buffer` frames. A slow client gets a `slow-client`
/// error frame and a store failure a `store-unavailable` one. The client resumes from the last cursor it received.
pub(crate) fn stream(writer: &mut impl Write, env: &Env, since: i64, limits: Limits) {
    run(writer, env, since, limits, &Arc::new(Shared::default()));
}

/// [`stream`] with the shared flags supplied by the caller.
fn run(writer: &mut impl Write, env: &Env, since: i64, limits: Limits, shared: &Arc<Shared>) {
    let _close = CloseOnDrop(Arc::clone(shared));
    let (tx, rx) = sync_channel(limits.buffer);
    let env = env.clone();
    let poller = Arc::clone(shared);
    thread::spawn(move || poll(&env, since, limits, &tx, &poller));
    let mut last = since;
    for frame in rx {
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
    }
}

/// Write one frame and flush it.
fn send(writer: &mut impl Write, frame: &ServerFrame) -> std::io::Result<()> {
    writer.write_all(frame.to_line().as_bytes())?;
    writer.flush()
}

/// Read events after `since` forever, queueing them without ever waiting on the client.
///
/// The cursor advances only for events actually queued, so a full buffer leaves
/// no gap. A store failure is recorded in `shared` for the connection to report.
fn poll(env: &Env, since: i64, limits: Limits, tx: &SyncSender<ServerFrame>, shared: &Shared) {
    // `Moor` is not `Send`, so it is built here rather than handed to the thread.
    let moor = match Moor::from_env(env) {
        Ok(moor) => moor,
        Err(e) => return shared.fail(e.error.to_string()),
    };
    let mut cursor = since;
    while !shared.closed.load(Ordering::Acquire) {
        let batch = match moor.events_after(cursor, limits.batch) {
            Ok(batch) => batch,
            Err(reason) => return shared.fail(reason),
        };
        if batch.is_empty() {
            thread::sleep(limits.poll);
            continue;
        }
        for frame in batch {
            let id = match &frame {
                ServerFrame::Event { cursor, .. } => *cursor,
                _ => cursor,
            };
            match tx.try_send(frame) {
                Ok(()) => cursor = id,
                Err(TrySendError::Full(_)) => {
                    shared.overflow.store(true, Ordering::Release);
                    return;
                }
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
    }
}

#[cfg(test)]
mod tests;
