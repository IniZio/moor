//! Event watches that resume from their last cursor across a controller restart.

use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use moor_core::controller::{ClientFrame, FrameErrorKind, ServerFrame, WatchFilter};
use moor_core::{BoxId, SessionId};

use super::{redial, unexpected, Attempt, Client, ClientError, Conn, WallClock};

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
        let dialed = client
            .connect()
            .and_then(|stream| dial(&client, stream, since, &filter, follow));
        let conn = match dialed {
            Attempt::Done(conn) => conn,
            Attempt::Refused(e) | Attempt::Unrun(e) | Attempt::Failed(e) => return Err(e),
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

    /// Replaces the connection after `restarting`; `Ok(false)` when stopped meanwhile. Each
    /// new connection is handed to the stop handle before its hello, so a stop also ends a
    /// dial that waits on a held socket.
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
            &WallClock,
            || {
                let Some(restarting) = first.take() else {
                    return client
                        .connect()
                        .and_then(|stream| stop.admit(stream))
                        .and_then(|stream| dial(client, stream, cursor, filter, follow));
                };
                Attempt::Unrun(restarting)
            },
            |pause, _| stop.sleep(pause),
        );
        if stop.stopped() {
            return Ok(false);
        }
        self.conn = redialed?;
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

/// Completes the hello on `stream` and sends the watch on it.
fn dial(
    client: &Client,
    stream: UnixStream,
    since: i64,
    filter: &WatchFilter,
    follow: bool,
) -> Attempt<Conn, ClientError> {
    client.hello(stream).and_then(|mut conn| {
        let frame = ClientFrame::Watch {
            since,
            boxes: filter.boxes.clone(),
            kinds: filter.kinds.clone(),
            parent: filter.parent,
            follow,
            identity: client.identity.clone(),
        };
        match conn
            .send(&frame)
            .and_then(|()| Ok(conn.writer.set_read_timeout(None)?))
        {
            Ok(()) => Attempt::Done(conn),
            Err(e) => Attempt::Failed(e),
        }
    })
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

    /// Sleeps `pause`, or less when stopped meanwhile.
    fn sleep(&self, pause: Duration) {
        let _ = self
            .shared
            .1
            .wait_timeout_while(self.lock(), pause, |s| !s.stopped)
            .unwrap_or_else(PoisonError::into_inner);
    }

    /// Points the handle at `stream` and hands it back; once stopped, shuts it and fails the
    /// dial. This is the one check that ends a resume, so a stop that lands during a pause
    /// or a dial is never missed.
    fn admit(&self, stream: UnixStream) -> Attempt<UnixStream, ClientError> {
        let handle = match stream.try_clone() {
            Ok(handle) => handle,
            Err(e) => return Attempt::Failed(e.into()),
        };
        let mut state = self.lock();
        if state.stopped {
            drop(state);
            let _ = stream.shutdown(Shutdown::Both);
            return Attempt::Failed(ClientError::Protocol("the watch was stopped".to_owned()));
        }
        state.stream = handle;
        drop(state);
        Attempt::Done(stream)
    }
}
