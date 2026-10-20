//! A scripted controller on a Unix socket. It answers the hello, serves each watch with an
//! empty backlog caught up to [`BACKLOG_END`], answers each request from a per-command script
//! or a Box table, and records what it is sent.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use moor_core::controller::{parse_frame, ClientFrame, FrameErrorKind, ServerFrame};
use moor_core::request::Request;
use moor_core::response::{ErrorKind, ErrorView, Response};
use serde_json::json;

/// Cursor every backlog watch is caught up to; a live watch must resume from it.
pub(crate) const BACKLOG_END: i64 = 41;
/// Longest a wait polls before the test fails.
const DEADLINE: Duration = Duration::from_secs(10);
/// Pause between polls.
const POLL: Duration = Duration::from_millis(20);

/// One scripted answer to a request.
pub(crate) enum Scripted {
    /// Send this envelope as the response.
    Reply(Response),
    /// Close the connection without answering.
    Hangup,
    /// Keep the connection open and never answer.
    Hold,
}

/// Everything clients sent, in arrival order.
#[derive(Default)]
pub(crate) struct Seen {
    /// Protocol of each hello.
    pub(crate) hellos: Vec<u32>,
    /// `since` and `follow` of each watch.
    pub(crate) watches: Vec<(i64, bool)>,
    /// Each request.
    pub(crate) requests: Vec<Request>,
}

/// State shared with the connection threads.
struct Inner {
    /// Protocol this controller speaks.
    protocol: u32,
    /// What clients sent.
    seen: Seen,
    /// Answers still to give, by request command.
    scripts: HashMap<String, VecDeque<Scripted>>,
    /// Answer every watch with a store error, as a controller that cannot read its events does.
    fail_watches: bool,
    /// Names of the Boxes `start` made, in order, once the Box table is on.
    boxes: Option<Vec<String>>,
}

/// A running scripted controller; it serves until the test process exits.
pub(crate) struct FakeController {
    /// State shared with the connection threads.
    inner: Arc<Mutex<Inner>>,
}

impl FakeController {
    /// Binds `socket` and serves clients that speak `protocol`.
    pub(crate) fn serve(socket: &Path, protocol: u32) -> Self {
        let listener = UnixListener::bind(socket).unwrap();
        let inner = Arc::new(Mutex::new(Inner {
            protocol,
            seen: Seen::default(),
            scripts: HashMap::new(),
            fail_watches: false,
            boxes: None,
        }));
        let shared = Arc::clone(&inner);
        thread::spawn(move || {
            for stream in listener.incoming().map_while(Result::ok) {
                let inner = Arc::clone(&shared);
                thread::spawn(move || serve(stream, &inner));
            }
        });
        Self { inner }
    }

    /// Queues `answer` for the next request whose command is `cmd`.
    pub(crate) fn script(&self, cmd: &str, answer: Scripted) {
        self.lock()
            .scripts
            .entry(cmd.to_owned())
            .or_default()
            .push_back(answer);
    }

    /// Answers every later watch with a store error.
    pub(crate) fn fail_watches(&self) {
        self.lock().fail_watches = true;
    }

    /// Answers unscripted `start` and `show` requests from a table of Boxes, as the real
    /// controller does: `start` makes the named Box or attaches to it, and `show` names it
    /// or answers `no Box <name>`.
    pub(crate) fn keep_boxes(&self) {
        self.lock().boxes.get_or_insert_with(Vec::new);
    }

    /// Names of the Boxes `start` made, in order.
    pub(crate) fn boxes(&self) -> Vec<String> {
        self.lock().boxes.clone().unwrap_or_default()
    }

    /// Reads what clients sent so far.
    pub(crate) fn seen<T>(&self, read: impl FnOnce(&Seen) -> T) -> T {
        read(&self.lock().seen)
    }

    /// Polls until `done` holds for what clients sent; fails the test after the deadline.
    pub(crate) fn wait_until(&self, what: &str, done: impl Fn(&Seen) -> bool) {
        let mut waited = Duration::ZERO;
        while !self.seen(&done) {
            assert!(waited < DEADLINE, "controller never saw {what}");
            thread::sleep(POLL);
            waited += POLL;
        }
    }

    /// Locks the shared state.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap()
    }
}

/// A failed envelope for `cmd` carrying `message`.
pub(crate) fn failed(cmd: &str, message: &str) -> Response {
    Response::err(
        cmd,
        ErrorView {
            kind: ErrorKind::Failed,
            message: message.to_owned(),
            hints: Vec::new(),
            context: Vec::new(),
        },
    )
}

/// Serves one connection until the client leaves or a script hangs up.
fn serve(stream: UnixStream, inner: &Mutex<Inner>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    let mut line = String::new();
    while reader.read_line(&mut line).unwrap_or(0) > 0 {
        let next = answer(line.trim_end(), &mut inner.lock().unwrap());
        line.clear();
        let frame = match next {
            Next::Send(frame) => frame,
            Next::Wait => continue,
            Next::Close => return,
        };
        if writer.write_all(frame.to_line().as_bytes()).is_err() {
            return;
        }
        if matches!(
            frame,
            ServerFrame::CaughtUp { .. } | ServerFrame::Error { .. }
        ) {
            return;
        }
    }
}

/// What a connection does after one client line.
enum Next {
    /// Send this frame; a caught-up or error frame then closes the connection.
    Send(ServerFrame),
    /// Send nothing and keep the connection, as a live watch does.
    Wait,
    /// Close the connection.
    Close,
}

/// How the controller answers one client line.
fn answer(line: &str, inner: &mut Inner) -> Next {
    let frame = match parse_frame(line) {
        Ok(frame) => frame,
        Err(error) => return Next::Send(error),
    };
    match frame {
        ClientFrame::Hello { protocol } => {
            inner.seen.hellos.push(protocol);
            Next::Send(if protocol == inner.protocol {
                ServerFrame::Hello { protocol }
            } else {
                ServerFrame::Error {
                    id: None,
                    kind: FrameErrorKind::UnsupportedProtocol,
                    message: format!(
                        "client speaks protocol {protocol}, controller speaks {}",
                        inner.protocol
                    ),
                }
            })
        }
        ClientFrame::Watch { since, follow, .. } => {
            inner.seen.watches.push((since, follow));
            if inner.fail_watches {
                Next::Send(ServerFrame::Error {
                    id: None,
                    kind: FrameErrorKind::StoreUnavailable,
                    message: "cannot read the event store".to_owned(),
                })
            } else if follow {
                Next::Wait
            } else {
                Next::Send(ServerFrame::CaughtUp {
                    cursor: BACKLOG_END,
                })
            }
        }
        ClientFrame::Request { id, request } => {
            let cmd = command(&request);
            let next = match inner.scripts.get_mut(&cmd).and_then(VecDeque::pop_front) {
                Some(Scripted::Reply(envelope)) => {
                    Next::Send(ServerFrame::Response { id, envelope })
                }
                Some(Scripted::Hangup) => Next::Close,
                Some(Scripted::Hold) => Next::Wait,
                None => Next::Send(ServerFrame::Response {
                    id,
                    envelope: inner
                        .boxes
                        .as_mut()
                        .and_then(|boxes| from_table(&request, boxes))
                        .unwrap_or_else(|| failed(&cmd, &format!("no script for {cmd}"))),
                }),
            };
            inner.seen.requests.push(*request);
            next
        }
    }
}

/// The Box table's answer to a `start` or `show`; `None` for any other request.
fn from_table(request: &Request, boxes: &mut Vec<String>) -> Option<Response> {
    let (cmd, name) = match request {
        Request::Start(start) => ("start", start.name.as_str()),
        Request::Show(show) => ("show", show.box_name.as_ref()?.as_str()),
        _ => return None,
    };
    let found = boxes.iter().position(|b| b == name);
    let index = match (cmd, found) {
        (_, Some(index)) => index,
        ("start", None) => {
            boxes.push(name.to_owned());
            boxes.len() - 1
        }
        _ => return Some(failed(cmd, &format!("no Box {name}"))),
    };
    let value = json!({"box": {"id": index + 1, "name": name, "branch": format!("moor/{name}")}});
    Some(Response::ok(cmd, Some(value)))
}

/// The wire command name of `request`, such as `start`.
fn command(request: &Request) -> String {
    serde_json::to_value(request).unwrap()["cmd"]
        .as_str()
        .unwrap()
        .to_owned()
}
