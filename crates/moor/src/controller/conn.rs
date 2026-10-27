//! One controller connection: reads client frames and answers each in order.

use std::io::{BufRead, Write};
use std::sync::Arc;

use moor_core::controller::{
    parse_frame, write_bytes, ClientFrame, FrameErrorKind, ServerFrame, WatchFilter, PROTOCOL,
};
use moor_core::request::{Grant, UserCmd};
use moor_core::user::{Caller, Identity, Viewer};
use moor_core::{ArtifactReadReq, ErrorKind, Request, Response, RunReq};

use super::drain::Admitted;
use super::hangup::Hangup;
use super::progress::{respond_with_progress, wait_unless_draining, Sink, INTERVAL};
use super::watch;
use crate::env::Env;
use crate::error::CommandError;
use crate::vault::Vault;
use crate::Moor;

mod attach;
mod frames;

use attach::attach;
use frames::{hello_reply, hello_required, stray};

/// Whom one connection speaks for, the vault key its user requests use, and its place in the
/// drain that ends it.
pub(crate) struct Peer {
    /// Whom the connection speaks for, or why that is unknown.
    pub(crate) caller: Result<Caller, String>,
    /// The vault key.
    pub(crate) vault: Arc<Vault>,
    /// Tells when the client goes away, which cancels its run; none when it cannot be watched.
    pub(crate) hangup: Option<Hangup>,
    /// The connection as the controller's drain sees it.
    pub(crate) conn: Admitted,
}

#[cfg(test)]
impl Peer {
    /// The operator at uid 1000 on the controller socket, with a vault key in `dir`.
    pub(crate) fn local_in(dir: &std::path::Path) -> Self {
        std::fs::create_dir_all(dir).expect("vault dir");
        let vault = Vault::load_or_create(&dir.join("vault.key")).expect("vault");
        Self {
            caller: Ok(Caller::Operator(1000)),
            vault: Arc::new(vault),
            hangup: None,
            conn: Admitted::unwatched(),
        }
    }
}

/// Serve frames from `reader` until EOF, an I/O error on either side, or the peer's drain.
///
/// Frame-level problems get an error frame and leave the connection open.
pub(crate) fn handle(reader: impl BufRead + Send, writer: impl Write, env: &Env, peer: &Peer) {
    serve(
        reader,
        writer,
        &peer.conn,
        |id, request, link| {
            let cmd = request.cmd();
            match request.split_identity() {
                Ok((asserted, request)) => answer(env, peer, id, asserted.as_ref(), request, link),
                Err(why) => {
                    Some(CommandError::new(ErrorKind::Refused, why).into_response(cmd, vec![]))
                }
            }
        },
        |writer, since, filter, follow, asserted| {
            let ask = watch::Ask {
                since,
                filter: filter.clone(),
                follow,
                viewer: Moor::from_env(env)
                    .map_or(Viewer::Nobody, |m| m.watcher(&peer.caller, asserted)),
            };
            watch::stream_draining(
                writer,
                env,
                ask,
                watch::Limits::default(),
                peer.conn.drain(),
            );
        },
    );
}

/// Answer one request that asserts `asserted`, once [`Moor::authorize`] lets its caller act
/// on every Box it names; `None` when the drain began before a wait ended.
fn answer(
    env: &Env,
    peer: &Peer,
    id: &str,
    asserted: Option<&Identity>,
    request: Request,
    (input, out): Duplex<'_>,
) -> Option<Response> {
    let cmd = request.cmd();
    if matches!(request, Request::Version {}) {
        return Some(Moor::version());
    }
    let grant =
        match Moor::from_env(env).and_then(|m| m.authorize(&peer.caller, asserted, &request)) {
            Ok(grant) => grant,
            Err(e) => return Some(e.into_response(cmd, vec![])),
        };
    match request {
        Request::ArtifactRead(req) => Some(read_artifact(env, grant, id, &req, out)),
        Request::Attach(req) => Some(attach(env, peer, grant, id, &req, (input, out))),
        request => match request.into_user() {
            Ok((_, cmd)) => Some(answer_user(env, peer, asserted, &cmd)),
            Err(request) => match requester(peer, asserted, &request) {
                Ok(who) => {
                    let acting = Acting { who, grant };
                    respond_unless_draining(env, acting, id, *request, out, peer)
                }
                Err(e) => Some(e.into_response(request.cmd(), vec![])),
            },
        },
    }
}

/// Answer `request`, writing its frames to `out`; `None` when `peer`'s drain began before a wait
/// ended. Every other request runs to its end.
fn respond_unless_draining(
    env: &Env,
    acting: Acting,
    id: &str,
    request: Request,
    out: &mut dyn Write,
    peer: &Peer,
) -> Option<Response> {
    match request {
        request @ (Request::Wait(_) | Request::WaitEvents(_)) => {
            let (cmd, env) = (request.cmd(), env.clone());
            wait_unless_draining(cmd, id, INTERVAL, out, peer.conn.drain(), move || {
                respond(&env, acting, request)
            })
        }
        request => Some(respond_with_progress(
            env,
            acting,
            peer.hangup.as_ref(),
            id,
            request,
            INTERVAL,
            out,
        )),
    }
}

/// [`handle`] with the engine behind `answer` and the event stream behind `watch`,
/// so tests can serve over fakes. `answer` may write frames for the request before its
/// response, and returns `None` when `drain` began before it could finish; the client then
/// gets a `restarting` frame. A request being answered keeps `conn` readable through the
/// drain until its answer is ready. Once the drain of `conn` has begun no further line is
/// answered: the connection ends with a `restarting` frame that names no request, which also
/// answers a request half read, unread, or written after the read side shut.
pub(super) fn serve<W: Write>(
    mut reader: impl BufRead + Send,
    mut writer: W,
    conn: &Admitted,
    answer: impl Fn(&str, Request, Duplex<'_>) -> Option<Response>,
    watch: impl Fn(&mut W, i64, &WatchFilter, bool, Option<&Identity>),
) {
    let mut handshaken = false;
    let mut line = String::new();
    let mut last = false;
    loop {
        line.clear();
        if !matches!(reader.read_line(&mut line), Ok(n) if n > 0) {
            break;
        }
        if conn.drain().begun() {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        let reply = match parse_frame(&line) {
            Err(error) => error,
            Ok(ClientFrame::Hello { protocol }) => {
                handshaken |= protocol == PROTOCOL;
                hello_reply(protocol)
            }
            Ok(frame) if !handshaken && !matches!(frame, ClientFrame::Hello { .. }) => {
                hello_required(frame)
            }
            Ok(frame @ ClientFrame::Watch { .. }) => {
                start_watch(&mut writer, frame, &watch);
                return;
            }
            Ok(ClientFrame::Bytes { id, .. } | ClientFrame::Resize { id, .. }) => {
                send(&mut writer, &stray(id));
                return;
            }
            Ok(ClientFrame::Request { id, request }) => {
                let reply;
                (reply, last) =
                    request_reply(conn, &answer, id, *request, (&mut reader, &mut writer));
                reply
            }
        };
        if !send(&mut writer, &reply) || last {
            return;
        }
    }
    if conn.drain().begun() {
        send(&mut writer, &restarting());
    }
}

/// Hands a watch `frame` to `watch`, which streams until the connection ends.
fn start_watch<W: Write>(
    writer: &mut W,
    frame: ClientFrame,
    watch: &impl Fn(&mut W, i64, &WatchFilter, bool, Option<&Identity>),
) {
    if let ClientFrame::Watch {
        since,
        boxes,
        kinds,
        parent,
        follow,
        identity,
    } = frame
    {
        let filter = WatchFilter {
            boxes,
            kinds,
            parent,
        };
        watch(writer, since, &filter, follow, identity.as_ref());
    }
}

/// The reply to `request` sent as `id`, and whether it is the connection's last: an `attach`
/// that ran hands the connection to its relay.
fn request_reply(
    conn: &Admitted,
    answer: &impl Fn(&str, Request, Duplex<'_>) -> Option<Response>,
    id: String,
    request: Request,
    duplex: Duplex<'_>,
) -> (ServerFrame, bool) {
    let attach = matches!(request, Request::Attach(_));
    match conn
        .answering()
        .and_then(|_answering| answer(&id, request, duplex))
    {
        Some(envelope) => (ServerFrame::Response { id, envelope }, attach),
        None => (
            ServerFrame::Error {
                id: Some(id),
                kind: FrameErrorKind::Restarting,
                message: "the controller is restarting before the request finished; send it again"
                    .to_owned(),
            },
            false,
        ),
    }
}

/// The frame that ends a connection the drain closed: no request it did not answer ran.
pub(super) fn restarting() -> ServerFrame {
    ServerFrame::Error {
        id: None,
        kind: FrameErrorKind::Restarting,
        message: "the controller is restarting; send any unanswered request again".to_owned(),
    }
}

/// Write one frame line and flush; false once the client is gone.
pub(super) fn send(writer: &mut (impl Write + ?Sized), frame: &ServerFrame) -> bool {
    writer.write_all(frame.to_line().as_bytes()).is_ok() && writer.flush().is_ok()
}

/// Stream an Artifact's bytes as bytes frames for request `id`, then answer.
fn read_artifact(
    env: &Env,
    grant: Grant,
    id: &str,
    req: &ArtifactReadReq,
    out: &mut dyn Write,
) -> Response {
    match Moor::from_env(env) {
        Ok(moor) => moor
            .granted(grant)
            .read_artifact(req, &mut |data| write_bytes(out, id, data)),
        Err(e) => e.into_response("artifact-read", vec![]),
    }
}

/// The client's side of one connection: its frames, and where its answers go.
pub(super) type Duplex<'a> = (&'a mut (dyn BufRead + Send), &'a mut dyn Write);

/// Run a user request as the identity this connection may assert.
fn answer_user(env: &Env, peer: &Peer, asserted: Option<&Identity>, cmd: &UserCmd) -> Response {
    match Moor::from_env(env) {
        Ok(moor) => moor.call_user(&peer.caller, asserted, &peer.vault, cmd),
        Err(e) => e.into_response(cmd.cmd(), vec![]),
    }
}

/// The identity a `start`, `start-needs` or `env` acts as: the one asserted, checked against
/// this connection's socket. `env` previews a start, so it reads a repo as the same user. Other
/// requests act as no one.
fn requester(
    peer: &Peer,
    asserted: Option<&Identity>,
    request: &Request,
) -> Result<Option<Identity>, CommandError> {
    if !matches!(
        request,
        Request::Start(_) | Request::StartNeeds(_) | Request::Env(_)
    ) {
        return Ok(None);
    }
    peer.caller
        .as_ref()
        .map_err(Clone::clone)
        .and_then(|caller| caller.resolve(asserted))
        .map(Some)
        .map_err(|why| CommandError::new(ErrorKind::Refused, why))
}

/// Whom one request acts as, and what it may see.
pub(crate) struct Acting {
    /// The identity a `start`, `start-needs` or `env` acts as; none for other requests.
    pub(crate) who: Option<Identity>,
    /// What the request may see.
    pub(crate) grant: Grant,
}

/// The operator acting as no one, as the tests' controller socket does.
#[cfg(test)]
pub(crate) const fn operator() -> Acting {
    Acting {
        who: None,
        grant: Grant::operator(),
    }
}

/// Run `request` against a fresh engine acting as `acting`, as `moor call` does.
///
/// `moor call` forces JSON output, so the run-output inheritance the CLI applies
/// to other invocations is a no-op there and is not applied here.
pub(crate) fn respond(env: &Env, acting: Acting, request: Request) -> Response {
    let cmd = request.cmd();
    if matches!(request, Request::Version {}) {
        return Moor::version();
    }
    match Moor::from_env(env) {
        Ok(moor) => {
            let moor = moor.granted(acting.grant);
            match acting.who {
                Some(who) => moor.acting_as(who).call(request),
                None => moor.call(request),
            }
        }
        Err(e) => e.into_response(cmd, vec![]),
    }
}

/// Run `run` against a fresh engine, passing the command's output to `out` as it runs.
pub(crate) fn respond_run(env: &Env, grant: Grant, req: &RunReq, out: Sink<'_>) -> Response {
    match Moor::from_env(env) {
        Ok(moor) => moor.granted(grant).run_streamed(req, out),
        Err(e) => e.into_response("run", vec![]),
    }
}

#[cfg(test)]
#[path = "secret/tests.rs"]
mod secret;
#[cfg(test)]
#[path = "conn_tests.rs"]
mod tests;
