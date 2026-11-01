//! A scripted controller on a Unix socket: it answers the hello, `enroll` and `show`, ends a
//! backlog watch caught up at [`TIP`] or the last event, and sends each event to every live
//! watch. It records what it is sent.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use moor_core::controller::{parse_frame, ClientFrame, ServerFrame};
use moor_core::request::Request;
use moor_core::response::Response;
use moor_core::{BoxId, SessionId};
use serde_json::{json, Value};

/// Cursor a backlog watch is caught up to while no event was emitted.
pub(crate) const TIP: i64 = 10;

/// One watch a client opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Opened {
    /// The cursor it resumes after.
    pub(crate) since: i64,
    /// Whether it follows.
    pub(crate) follow: bool,
    /// The kinds it asked for.
    pub(crate) kinds: Option<Vec<String>>,
    /// The identity it asserted.
    pub(crate) identity: Option<String>,
}

/// What clients sent.
#[derive(Default)]
pub(crate) struct Seen {
    /// Every watch, in order.
    pub(crate) watches: Vec<Opened>,
    /// The command and asserted identity of every request, in order.
    pub(crate) requests: Vec<(String, Option<String>)>,
}

/// State shared with the connection threads.
#[derive(Default)]
struct Inner {
    /// What clients sent.
    seen: Seen,
    /// Every event emitted, oldest first.
    events: Vec<ServerFrame>,
    /// Each live watch.
    live: Vec<UnixStream>,
}

/// A running scripted controller.
pub(crate) struct FakeController {
    /// State shared with the connection threads.
    inner: Arc<Mutex<Inner>>,
}

impl FakeController {
    /// Binds `socket` and serves it until the test ends.
    pub(crate) fn serve(socket: &Path) -> Self {
        let listener = UnixListener::bind(socket).unwrap();
        let inner = Arc::new(Mutex::new(Inner::default()));
        let shared = Arc::clone(&inner);
        thread::spawn(move || {
            for stream in listener.incoming().map_while(Result::ok) {
                let inner = Arc::clone(&shared);
                thread::spawn(move || serve(stream, &inner));
            }
        });
        Self { inner }
    }

    /// Stores an event of `kind` for Box 7 `app/fix` and sends it to every live watch.
    pub(crate) fn emit(&self, kind: &str, payload: Value) -> i64 {
        let mut inner = self.lock();
        let cursor = TIP + 1 + i64::try_from(inner.events.len()).unwrap();
        let frame = ServerFrame::Event {
            cursor,
            r#box: Some("app/fix".to_owned()),
            box_id: Some(BoxId::new(7)),
            session: Some(SessionId::new(3)),
            kind: kind.to_owned(),
            payload,
            ts: 1_700_000_000_000 + cursor,
        };
        let line = frame.to_line();
        inner.events.push(frame);
        inner
            .live
            .retain_mut(|stream| stream.write_all(line.as_bytes()).is_ok());
        cursor
    }

    /// Reads what clients sent so far.
    pub(crate) fn seen<T>(&self, read: impl FnOnce(&Seen) -> T) -> T {
        read(&self.lock().seen)
    }

    /// Waits until `done` holds for what clients sent; fails the test after 10 s.
    pub(crate) fn wait_until(&self, what: &str, done: impl Fn(&Seen) -> bool) {
        for _ in 0..500 {
            if self.seen(&done) {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("controller never saw {what}");
    }

    /// Locks the shared state.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap()
    }
}

/// Serves one connection until the client leaves, or a backlog watch is caught up.
fn serve(stream: UnixStream, inner: &Mutex<Inner>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    let mut line = String::new();
    while reader.read_line(&mut line).unwrap_or(0) > 0 {
        let frame = parse_frame(line.trim_end()).unwrap();
        line.clear();
        let mut inner = inner.lock().unwrap();
        let reply = match frame {
            ClientFrame::Hello { protocol } => ServerFrame::Hello { protocol },
            ClientFrame::Request { id, request } => {
                let envelope = answer(*request, &mut inner);
                ServerFrame::Response { id, envelope }
            }
            ClientFrame::Watch {
                since,
                kinds,
                follow,
                identity,
                ..
            } => {
                inner.seen.watches.push(Opened {
                    since,
                    follow,
                    kinds,
                    identity: identity.map(|i| i.to_string()),
                });
                let backlog: String = inner
                    .events
                    .iter()
                    .filter(|f| matches!(f, ServerFrame::Event { cursor, .. } if *cursor > since))
                    .map(ServerFrame::to_line)
                    .collect();
                if follow {
                    if writer.write_all(backlog.as_bytes()).is_ok() {
                        inner.live.push(writer.try_clone().unwrap());
                    }
                    continue;
                }
                let tip = TIP + i64::try_from(inner.events.len()).unwrap();
                let _ = writer.write_all(backlog.as_bytes());
                let _ =
                    writer.write_all(ServerFrame::CaughtUp { cursor: tip }.to_line().as_bytes());
                return;
            }
            ClientFrame::Bytes { .. } | ClientFrame::Resize { .. } => return,
        };
        drop(inner);
        if writer.write_all(reply.to_line().as_bytes()).is_err() {
            return;
        }
    }
}

/// The answer to `request`, recorded with the identity it asserted.
fn answer(request: Request, inner: &mut Inner) -> Response {
    let (identity, request) = match request {
        Request::As(act) => (Some(act.identity.to_string()), *act.request),
        bare => (None, bare),
    };
    let cmd = match &request {
        Request::Enroll(enroll) => {
            assert_eq!(
                enroll.email.as_ref().map(|e| e.as_str().to_owned()),
                identity
                    .as_deref()
                    .and_then(|i| i.strip_prefix("notify:"))
                    .map(str::to_owned),
                "enroll sends the hook's email"
            );
            "enroll"
        }
        Request::Show(_) => "show",
        other => panic!("unexpected request {other:?}"),
    };
    inner.seen.requests.push((cmd.to_owned(), identity));
    let value = match cmd {
        "show" => json!({"box": {
            "name": "app/fix",
            "lifecycle": "ready",
            "session_state": "idle",
            "said": {"text": "Opened the PR.", "at": "2026-11-01T00:00:00Z"},
        }}),
        _ => json!({}),
    };
    Response::ok(cmd, Some(value))
}
