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
pub(crate) fn handle(reader: impl BufRead, writer: impl Write, env: &Env, peer: &Peer) {
    serve(
        reader,
        writer,
        &peer.conn,
        |id, request, out| {
            let cmd = request.cmd();
            match request.split_identity() {
                Ok((asserted, request)) => answer(env, peer, id, asserted.as_ref(), request, out),
                Err(why) => {
                    Some(CommandError::new(ErrorKind::Refused, why).into_response(cmd, vec![]))
                }
            }
        },
        |writer, since, filter, follow| {
            let ask = watch::Ask {
                since,
                filter: filter.clone(),
                follow,
                viewer: Moor::from_env(env).map_or(Viewer::Nobody, |m| m.watcher(&peer.caller)),
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
    out: &mut dyn Write,
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
    reader: impl BufRead,
    mut writer: W,
    conn: &Admitted,
    answer: impl Fn(&str, Request, &mut dyn Write) -> Option<Response>,
    watch: impl Fn(&mut W, i64, &WatchFilter, bool),
) {
    let mut handshaken = false;
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if conn.drain().begun() {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        let reply = match parse_frame(&line) {
            Err(error) => error,
            Ok(ClientFrame::Hello { protocol }) if protocol == PROTOCOL => {
                handshaken = true;
                ServerFrame::Hello { protocol: PROTOCOL }
            }
            Ok(ClientFrame::Hello { protocol }) => ServerFrame::Error {
                id: None,
                kind: FrameErrorKind::UnsupportedProtocol,
                message: format!("client speaks protocol {protocol}, controller speaks {PROTOCOL}"),
            },
            Ok(ClientFrame::Watch { .. }) if !handshaken => ServerFrame::Error {
                id: None,
                kind: FrameErrorKind::HelloRequired,
                message: "watch arrived before hello".to_owned(),
            },
            Ok(ClientFrame::Watch {
                since,
                boxes,
                kinds,
                follow,
            }) => {
                watch(&mut writer, since, &WatchFilter { boxes, kinds }, follow);
                return;
            }
            Ok(ClientFrame::Request { id, .. }) if !handshaken => ServerFrame::Error {
                message: format!("request {id} arrived before hello"),
                id: Some(id),
                kind: FrameErrorKind::HelloRequired,
            },
            Ok(ClientFrame::Request { id, request }) => match conn
                .answering()
                .and_then(|_answering| answer(&id, *request, &mut writer))
            {
                Some(envelope) => ServerFrame::Response { id, envelope },
                None => ServerFrame::Error {
                    id: Some(id),
                    kind: FrameErrorKind::Restarting,
                    message:
                        "the controller is restarting before the request finished; send it again"
                            .to_owned(),
                },
            },
        };
        if !send(&mut writer, &reply) {
            return;
        }
    }
    if conn.drain().begun() {
        send(&mut writer, &restarting());
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

/// Run a user request as the identity this connection may assert.
fn answer_user(env: &Env, peer: &Peer, asserted: Option<&Identity>, cmd: &UserCmd) -> Response {
    match Moor::from_env(env) {
        Ok(moor) => moor.call_user(&peer.caller, asserted, &peer.vault, cmd),
        Err(e) => e.into_response(cmd.cmd(), vec![]),
    }
}

/// The identity a `start` or `start-needs` acts as: the one asserted, checked against this
/// connection's socket. Other requests act as no one.
fn requester(
    peer: &Peer,
    asserted: Option<&Identity>,
    request: &Request,
) -> Result<Option<Identity>, CommandError> {
    if !matches!(request, Request::Start(_) | Request::StartNeeds(_)) {
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
    /// The identity a `start` acts as; none for other requests.
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
