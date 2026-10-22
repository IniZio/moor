//! Event watches that resume from their last cursor across a controller restart.

use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use moor_core::controller::{ClientFrame, FrameErrorKind, ServerFrame, WatchFilter};
use moor_core::{BoxId, SessionId};

use super::{redial, unexpected, Attempt, Client, ClientError, Conn};

/// One stored event delivered by a watch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRecord {
    /// The event's store id.
    pub cursor: i64,
    /// Box the event belongs to, when it has one.
    pub box_name: Option<String>,
    /// Id of the Box the event belongs to, when it has one.
    pub box_id: Option<BoxId>,
    /// Session the event belongs to, when known.
    pub session: Option<SessionId>,
    /// Event kind.
    pub kind: String,
    /// Kind-specific data.
    pub payload: serde_json::Value,
    /// Event timestamp.
    pub ts: i64,
}

/// One item of a watch stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchItem {
    /// A stored event.
    Event(EventRecord),
    /// The backlog is delivered; resume from `cursor`.
    CaughtUp {
        /// Last event id scanned.
        cursor: i64,
    },
}

/// A live watch; iterates events until the controller closes the stream.
///
/// A `restarting` end dials again within the client's redial budget and asks for the events
/// after the last cursor delivered, so none is repeated or skipped.
#[derive(Debug)]
pub struct Watch {
    /// Dials the replacement connection.
    client: Client,
    /// Which events to ask for.
    filter: WatchFilter,
    /// Whether the stream stays open after the backlog.
    follow: bool,
    /// The open connection.
    conn: Conn,
    /// Last event or caught-up cursor delivered; a resumed stream starts after it.
    cursor: i64,
    /// Set after an error or EOF so iteration ends; caught-up frames do not stop it.
    done: bool,
    /// Shared with every [`WatchStop`] handed out.
    stop: WatchStop,
}

impl Watch {
    /// Dials once and sends the watch; the caller decides how to retry a first dial.
    pub(super) fn open(
        client: Client,
        since: i64,
        filter: WatchFilter,
        follow: bool,
    ) -> Result<Self, ClientError> {
        let conn = match dial(&client, since, &filter, follow) {
            Attempt::Done(conn) => conn,
            Attempt::Unrun(e) | Attempt::Failed(e) => return Err(e),
        };
        let stop = WatchStop::new(conn.writer.try_clone()?);
        Ok(Self {
            client,
            filter,
            follow,
            conn,
            cursor: since,
            done: false,
            stop,
        })
    }

    /// A handle that ends this watch from another thread: a blocked or redialling `next()`
    /// returns `None`.
    #[must_use]
    pub fn stopper(&self) -> WatchStop {
        self.stop.clone()
    }

    /// Replaces the connection after `restarting`; `Ok(false)` when stopped meanwhile.
    fn resume(&mut self, restarting: ClientError) -> Result<bool, ClientError> {
        let mut first = Some(restarting);
        let (client, filter, follow, cursor, stop) = (
            &self.client,
            &self.filter,
            self.follow,
            self.cursor,
            &self.stop,
        );
        let redialed = redial(
            &client.redial,
            || {
                first
                    .take()
                    .map_or_else(|| dial(client, cursor, filter, follow), Attempt::Unrun)
            },
            |pause, _| stop.sleep(pause),
        );
        if stop.stopped() {
            return Ok(false);
        }
        let conn = redialed?;
        if !stop.swap(&conn)? {
            return Ok(false);
        }
        self.conn = conn;
        Ok(true)
    }

    /// Turns one frame into an item; `None` when the stream resumed on a new connection.
    fn item(&mut self, frame: ServerFrame) -> Option<Result<WatchItem, ClientError>> {
        match frame {
            ServerFrame::Event {
                cursor,
                r#box,
                box_id,
                session,
                kind,
                payload,
                ts,
            } => {
                self.cursor = cursor;
                Some(Ok(WatchItem::Event(EventRecord {
                    cursor,
                    box_name: r#box,
                    box_id,
                    session,
                    kind,
                    payload,
                    ts,
                })))
            }
            ServerFrame::CaughtUp { cursor } => {
                self.cursor = self.cursor.max(cursor);
                Some(Ok(WatchItem::CaughtUp { cursor }))
            }
            ServerFrame::Error {
                kind: FrameErrorKind::Restarting,
                message,
                ..
            } => {
                let restarting = ClientError::Frame {
                    kind: FrameErrorKind::Restarting,
                    message,
                };
                match self.resume(restarting) {
                    Ok(true) => None,
                    Ok(false) => {
                        self.done = true;
                        None
                    }
                    Err(e) => {
                        self.done = true;
                        Some(Err(e))
                    }
                }
            }
            other => {
                self.done = true;
                Some(Err(unexpected(other, "event or caught-up")))
            }
        }
    }
}

impl Iterator for Watch {
    type Item = Result<WatchItem, ClientError>;

    fn next(&mut self) -> Option<Self::Item> {
        while !self.done {
            match self.conn.read_frame() {
                Ok(Some(frame)) => {
                    if let Some(item) = self.item(frame) {
                        return Some(item);
                    }
                }
                Ok(None) => self.done = true,
                Err(e) => {
                    self.done = true;
                    return Some(Err(e));
                }
            }
        }
        None
    }
}

/// Opens a connection and sends the watch on it.
fn dial(
    client: &Client,
    since: i64,
    filter: &WatchFilter,
    follow: bool,
) -> Attempt<Conn, ClientError> {
    let mut conn = match client.open() {
        Attempt::Done(conn) => conn,
        other => return other,
    };
    let frame = ClientFrame::Watch {
        since,
        boxes: filter.boxes.clone(),
        kinds: filter.kinds.clone(),
        follow,
    };
    match conn
        .send(&frame)
        .and_then(|()| Ok(conn.writer.set_read_timeout(None)?))
    {
        Ok(()) => Attempt::Done(conn),
        Err(e) => Attempt::Failed(e),
    }
}

/// Ends a [`Watch`] from another thread, including one between connections.
#[derive(Debug, Clone)]
pub struct WatchStop {
    /// The stop flag and the live connection, with a condvar that wakes a redial pause.
    shared: Arc<(Mutex<Stopping>, Condvar)>,
}

/// State behind a [`WatchStop`].
#[derive(Debug)]
struct Stopping {
    /// Set by [`WatchStop::stop`].
    stopped: bool,
    /// A handle on the watch's current connection.
    stream: UnixStream,
}

impl WatchStop {
    /// A stop handle over the connection `stream` is a clone of.
    fn new(stream: UnixStream) -> Self {
        let stopping = Stopping {
            stopped: false,
            stream,
        };
        Self {
            shared: Arc::new((Mutex::new(stopping), Condvar::new())),
        }
    }

    /// Ends the watch: shuts its connection and stops any redial.
    pub fn stop(&self) {
        {
            let mut state = self.lock();
            state.stopped = true;
            let _ = state.stream.shutdown(Shutdown::Both);
        }
        self.shared.1.notify_all();
    }

    /// Locks the state, through a poisoned lock too.
    fn lock(&self) -> MutexGuard<'_, Stopping> {
        self.shared.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// True once stopped.
    fn stopped(&self) -> bool {
        self.lock().stopped
    }

    /// Sleeps `pause` unless stopped first; true when not stopped.
    fn sleep(&self, pause: Duration) -> bool {
        let (state, _) = self
            .shared
            .1
            .wait_timeout_while(self.lock(), pause, |s| !s.stopped)
            .unwrap_or_else(PoisonError::into_inner);
        let stopped = state.stopped;
        drop(state);
        !stopped
    }

    /// Points the handle at `conn`; false, with `conn` shut, when stopped already.
    fn swap(&self, conn: &Conn) -> Result<bool, ClientError> {
        let stream = conn.writer.try_clone()?;
        let mut state = self.lock();
        if state.stopped {
            drop(state);
            let _ = conn.writer.shutdown(Shutdown::Both);
            return Ok(false);
        }
        state.stream = stream;
        drop(state);
        Ok(true)
    }
}
