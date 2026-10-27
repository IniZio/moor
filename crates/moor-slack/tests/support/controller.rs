//! A scripted controller on a Unix socket. It answers the hello, serves each watch with the
//! events the test emitted, caught up to [`BACKLOG_END`] or the last event, sends each later
//! event to every live watch, answers each request from a per-command script or a Box table,
//! and records what it is sent.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use moor_core::controller::{parse_frame, ClientFrame, FrameErrorKind, ServerFrame};
use moor_core::request::Request;
use moor_core::response::{ErrorKind, ErrorView, Response};
use serde_json::json;

mod binding;
mod emit;

/// Cursor a backlog watch is caught up to while no event was emitted; a live watch must
/// resume from it.
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
    /// Answer with this envelope after the pause.
    Slow(Duration, Response),
    /// Act on the request as an unscripted one, then never answer, as a controller whose
    /// reply the front never read.
    Lost,
}

/// Everything clients sent, in arrival order.
#[derive(Default)]
pub(crate) struct Seen {
    /// Protocol of each hello.
    pub(crate) hellos: Vec<u32>,
    /// `since` and `follow` of each watch.
    pub(crate) watches: Vec<(i64, bool)>,
    /// Each request, with its `as` wrapper removed.
    pub(crate) requests: Vec<Request>,
    /// The identity each request asserted, in the order of `requests`; none for a bare one.
    pub(crate) acting: Vec<Option<String>>,
    /// The identity each watch asserted, in the order of `watches`.
    pub(crate) watchers: Vec<Option<String>>,
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
    /// Workdir of each host Session `open-host-session` opened, and whether it has ended;
    /// its id is its index plus 100.
    hosts: Vec<(String, bool)>,
    /// Every binding `bind-session` made.
    bindings: Vec<binding::Bound>,
    /// Every event emitted so far, oldest first, with the identity that owns its Box when it
    /// has one; cursors follow [`BACKLOG_END`].
    events: Vec<(Option<String>, ServerFrame)>,
    /// The identity and connection of each live watch.
    live: Vec<(Option<String>, UnixStream)>,
    /// How many events a backlog sends before it pauses, and for how long, once set.
    split: Option<(usize, Duration)>,
    /// Holds a file per Slack user with the lease `proxy-lease` grants that user; a user with
    /// none has no secret.
    leases: PathBuf,
    /// Whether `proxy-lease` and `proxy-status` find the front Proxy running.
    proxy_up: bool,
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
            hosts: Vec::new(),
            bindings: Vec::new(),
            events: Vec::new(),
            live: Vec::new(),
            split: None,
            leases: socket.parent().unwrap().join("leases"),
            proxy_up: true,
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

    /// Answers `proxy-lease` and `proxy-status` as a controller whose front Proxy is down.
    pub(crate) fn proxy_down(&self) {
        self.lock().proxy_up = false;
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

    /// Each host Session opened so far, by id, with its key and whether it has ended.
    pub(crate) fn hosts(&self) -> Vec<(i64, String, bool)> {
        let hosts = self.lock().hosts.clone();
        (100..)
            .zip(hosts)
            .map(|(id, (w, ended))| (id, w, ended))
            .collect()
    }

    /// Ends host Session `session` from outside the front, as moor-web does.
    pub(crate) fn end_host(&self, session: i64) {
        let index = usize::try_from(session - 100).unwrap();
        self.lock().hosts[index].1 = true;
    }

    /// Sends each later backlog in two writes `pause` apart: its first `events` events, then
    /// the rest and the caught-up frame, so the front reads it in more than one batch.
    pub(crate) fn split_backlogs(&self, events: usize, pause: Duration) {
        self.lock().split = Some((events, pause));
    }

    /// Ends every live watch, as a controller restart does; the front dials again and gets
    /// every event again.
    pub(crate) fn drop_watches(&self) {
        for (_, stream) in self.lock().live.drain(..) {
            let _ = stream.shutdown(Shutdown::Both);
        }
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
            detail: None,
        },
    )
}

/// Serves one connection until the client leaves or a script hangs up.
fn serve(stream: UnixStream, inner: &Mutex<Inner>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    let mut line = String::new();
    while reader.read_line(&mut line).unwrap_or(0) > 0 {
        let mut guard = inner.lock().unwrap();
        let next = answer(line.trim_end(), &mut guard);
        line.clear();
        let mut pause = Duration::ZERO;
        let frame = match next {
            Next::Send(frame) => frame,
            Next::Later(wait, frame) => {
                pause = wait;
                frame
            }
            Next::Backlog(since, watcher) => {
                let (parts, pause) = backlog(&guard, since, watcher.as_deref());
                drop(guard);
                let [first, rest] = parts;
                if writer.write_all(first.as_bytes()).is_ok() {
                    thread::sleep(pause);
                    let _ = writer.write_all(rest.as_bytes());
                }
                return;
            }
            Next::Follow(since, watcher) => {
                follow(&mut writer, &mut guard, since, watcher);
                continue;
            }
            Next::Wait => continue,
            Next::Close => return,
        };
        drop(guard);
        thread::sleep(pause);
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
    /// Send every event after this cursor that the identity carries, then the caught-up
    /// frame, then close.
    Backlog(i64, Option<String>),
    /// Send every event after this cursor that the identity carries, then keep the
    /// connection for later events.
    Follow(i64, Option<String>),
    /// Send this frame after the pause.
    Later(Duration, ServerFrame),
    /// Send nothing and keep the connection.
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
        ClientFrame::Watch {
            since,
            follow,
            identity,
            ..
        } => {
            let watcher = identity.map(|i| i.to_string());
            inner.seen.watches.push((since, follow));
            inner.seen.watchers.push(watcher.clone());
            if inner.fail_watches {
                Next::Send(ServerFrame::Error {
                    id: None,
                    kind: FrameErrorKind::StoreUnavailable,
                    message: "cannot read the event store".to_owned(),
                })
            } else if follow {
                Next::Follow(since, watcher)
            } else {
                Next::Backlog(since, watcher)
            }
        }
        ClientFrame::Request { id, request } => scripted(id, *request, inner),
        ClientFrame::Bytes { .. } | ClientFrame::Resize { .. } => Next::Close,
    }
}

/// How the controller answers request `request` sent as `id`, with its `as` wrapper taken
/// off and recorded.
fn scripted(id: String, request: Request, inner: &mut Inner) -> Next {
    let (acting, request) = match request {
        Request::As(act) => (Some(act.identity.to_string()), *act.request),
        bare => (None, bare),
    };
    let cmd = command(&request);
    let next = match inner.scripts.get_mut(&cmd).and_then(VecDeque::pop_front) {
        Some(Scripted::Reply(envelope)) => Next::Send(ServerFrame::Response { id, envelope }),
        Some(Scripted::Slow(pause, envelope)) => {
            Next::Later(pause, ServerFrame::Response { id, envelope })
        }
        Some(Scripted::Hangup) => Next::Close,
        Some(Scripted::Hold) => Next::Wait,
        Some(Scripted::Lost) => {
            unscripted(&request, inner);
            Next::Wait
        }
        None => Next::Send(ServerFrame::Response {
            id,
            envelope: proxy_cmd(&request, acting.as_deref(), inner)
                .unwrap_or_else(|| unscripted(&request, inner)),
        }),
    };
    inner.seen.requests.push(request);
    inner.seen.acting.push(acting);
    next
}

/// A backlog watch's events after `since` and its caught-up frame, in the two parts it is
/// written in, with the pause between them.
fn backlog(inner: &Inner, since: i64, watcher: Option<&str>) -> ([String; 2], Duration) {
    let cursor = BACKLOG_END + i64::try_from(inner.events.len()).unwrap();
    let mut lines: Vec<String> = events(inner, since, watcher)
        .map(ServerFrame::to_line)
        .collect();
    lines.push(ServerFrame::CaughtUp { cursor }.to_line());
    let (at, pause) = inner.split.unwrap_or((0, Duration::ZERO));
    let rest = lines.split_off(at.min(lines.len()));
    ([lines.concat(), rest.concat()], pause)
}

/// Sends the events after `since` on a live watch and keeps it for later events.
fn follow(writer: &mut UnixStream, inner: &mut Inner, since: i64, watcher: Option<String>) {
    let missed: String = events(inner, since, watcher.as_deref())
        .map(ServerFrame::to_line)
        .collect();
    if writer.write_all(missed.as_bytes()).is_ok() {
        inner.live.push((watcher, writer.try_clone().unwrap()));
    }
}

/// Every event after `since` that a watch asserting `watcher` carries, oldest first.
fn events<'a>(
    inner: &'a Inner,
    since: i64,
    watcher: Option<&'a str>,
) -> impl Iterator<Item = &'a ServerFrame> {
    inner
        .events
        .iter()
        .filter(move |(owner, _)| carries(owner.as_deref(), watcher))
        .map(|(_, frame)| frame)
        .filter(move |frame| matches!(frame, ServerFrame::Event { cursor, .. } if *cursor > since))
}

/// Whether a watch asserting `watcher` carries an event of a Box owned by `owner`: events
/// with no owner go to every watch.
fn carries(owner: Option<&str>, watcher: Option<&str>) -> bool {
    owner.is_none() || owner == watcher
}

/// The answer to a request no script covers: from the host Sessions, then the Box table.
fn unscripted(request: &Request, inner: &mut Inner) -> Response {
    user_cmd(request)
        .or_else(|| host_session(request, &mut inner.hosts))
        .or_else(|| binding::answer(request, &mut inner.hosts, &mut inner.bindings))
        .or_else(|| from_table(request, inner.boxes.as_mut()?))
        .unwrap_or_else(|| {
            let cmd = command(request);
            failed(&cmd, &format!("no script for {cmd}"))
        })
}

/// The answer to `proxy-lease`, `proxy-release` or `proxy-status` as `acting`; `None` for any
/// other request. A lease is granted only to a Slack user with a file in [`Inner::leases`], and
/// its token is what that file holds.
fn proxy_cmd(request: &Request, acting: Option<&str>, inner: &Inner) -> Option<Response> {
    let (cmd, value) = match request {
        Request::ProxyLease(_) if !inner.proxy_up => {
            ("proxy-lease", json!({"lease": "proxy_down"}))
        }
        Request::ProxyLease(_) => {
            let user = acting
                .and_then(|a| a.rsplit('/').next())
                .unwrap_or_default();
            let value = std::fs::read_to_string(inner.leases.join(user)).map_or_else(
                |_| json!({"lease": "no_secret", "name": "claude"}),
                |token| json!({"lease": "granted", "token": token.trim(), "expires_in_secs": 900}),
            );
            ("proxy-lease", value)
        }
        Request::ProxyRelease(_) => ("proxy-release", json!({"released": true})),
        Request::ProxyStatus(_) => ("proxy-status", json!({"running": inner.proxy_up})),
        _ => return None,
    };
    Some(Response::ok(cmd, Some(value)))
}

/// The answer to `enroll`, `secret-set`, `secret-delete` or `secret-names`; `None` for any
/// other request. The fake keeps no vault: a test reads what was asked from [`Seen`].
fn user_cmd(request: &Request) -> Option<Response> {
    let (cmd, value) = match request {
        Request::Enroll(_) => ("enroll", json!({"user": 1, "created": true})),
        Request::SecretSet(set) => ("secret-set", json!({"name": set.name, "changed": true})),
        Request::SecretDelete(del) => ("secret-delete", json!({"name": del.name, "changed": true})),
        Request::SecretNames(_) => ("secret-names", json!({"names": []})),
        _ => return None,
    };
    Some(Response::ok(cmd, Some(value)))
}

/// The Box table's answer to a `start` or `show`; `None` for any other request.
fn from_table(request: &Request, boxes: &mut Vec<String>) -> Option<Response> {
    let (cmd, name) = match request {
        Request::Start(start) => ("start", start.name.as_str().to_owned()),
        Request::Show(show) => ("show", show.target.as_ref()?.to_string()),
        _ => return None,
    };
    let found = boxes.iter().position(|b| *b == name);
    let index = match (cmd, found) {
        (_, Some(index)) => index,
        ("start", None) => {
            boxes.push(name.clone());
            boxes.len() - 1
        }
        _ => return Some(failed(cmd, &format!("no Box {name}"))),
    };
    let value = json!({"box": {"id": index + 1, "name": name, "branch": format!("moor/{name}")}});
    Some(Response::ok(cmd, Some(value)))
}

/// The answer to `sessions` or an `open-`, `update-` or `end-host-session` from the host
/// Sessions opened so far, as the real controller gives it; `None` for any other request.
fn host_session(request: &Request, hosts: &mut Vec<(String, bool)>) -> Option<Response> {
    let id = |index: usize| i64::try_from(index).unwrap() + 100;
    match request {
        Request::Sessions(_) => {
            let rows: Vec<_> = hosts
                .iter()
                .enumerate()
                .filter(|(_, (_, ended))| !ended)
                .map(|(i, (key, _))| json!({"session": id(i), "box": null, "workdir": workdir(id(i)), "key": key, "state": "working"}))
                .collect();
            Some(Response::ok("sessions", Some(json!({"sessions": rows}))))
        }
        Request::OpenHostSession(open) => {
            let key = open.key.clone().unwrap_or_default();
            let live = hosts
                .iter()
                .position(|(k, ended)| !ended && open.key.is_some() && *k == key);
            let index = live.unwrap_or_else(|| {
                hosts.push((key, false));
                hosts.len() - 1
            });
            let value =
                json!({"session": id(index), "workdir": workdir(id(index)), "state": "working"});
            Some(Response::ok("open-host-session", Some(value)))
        }
        Request::UpdateHostSession(update) => {
            let session = update.session.get();
            if let Err(message) = live_host(hosts, session) {
                return Some(refused("update-host-session", &message));
            }
            let value =
                json!({"session": session, "workdir": workdir(session), "state": "working"});
            Some(Response::ok("update-host-session", Some(value)))
        }
        Request::EndHostSession(end) => {
            let session = end.session.get();
            let (_, ended) = match live_host(hosts, session) {
                Ok(host) => host,
                Err(message) => return Some(refused("end-host-session", &message)),
            };
            *ended = true;
            let value = json!({"session": session, "workdir": workdir(session), "state": "ended"});
            Some(Response::ok("end-host-session", Some(value)))
        }
        _ => None,
    }
}

/// The workdir the controller gives host Session `session`.
fn workdir(session: i64) -> String {
    format!("/state/users/1/sessions/{session}")
}

/// The live host Session `session`, or the real controller's refusal text.
fn live_host(hosts: &mut [(String, bool)], session: i64) -> Result<&mut (String, bool), String> {
    let index = usize::try_from(session - 100).ok();
    let host = index
        .and_then(|i| hosts.get_mut(i))
        .ok_or_else(|| format!("no host Session {session}"))?;
    if host.1 {
        return Err(format!("host Session {session} has ended"));
    }
    Ok(host)
}

/// A refused envelope for `cmd` carrying `message`.
pub(crate) fn refused(cmd: &str, message: &str) -> Response {
    Response::err(
        cmd,
        ErrorView {
            kind: ErrorKind::Refused,
            message: message.to_owned(),
            hints: Vec::new(),
            context: Vec::new(),
            detail: None,
        },
    )
}

/// The wire command name of `request`, such as `start`.
fn command(request: &Request) -> String {
    serde_json::to_value(request).unwrap()["cmd"]
        .as_str()
        .unwrap()
        .to_owned()
}
