//! One controller connection: reads client frames and answers each in order.

use std::io::{BufRead, Write};
use std::sync::Arc;

use moor_core::controller::{
    parse_frame, write_bytes, ClientFrame, FrameErrorKind, ServerFrame, WatchFilter, PROTOCOL,
};
use moor_core::request::UserCmd;
use moor_core::user::{Caller, Identity};
use moor_core::{ArtifactReadReq, ErrorKind, Request, Response, RunReq};

use super::progress::{respond_with_progress, Sink, INTERVAL};
use super::watch;
use crate::env::Env;
use crate::error::CommandError;
use crate::vault::Vault;
use crate::Moor;

/// Whom one connection speaks for, and the vault key its user requests use.
pub(crate) struct Peer {
    /// Whom the connection speaks for, or why that is unknown.
    pub(crate) caller: Result<Caller, String>,
    /// The vault key.
    pub(crate) vault: Arc<Vault>,
}

#[cfg(test)]
impl Peer {
    /// The controller socket's peer uid 1000, with a vault key in `dir`.
    pub(crate) fn local_in(dir: &std::path::Path) -> Self {
        std::fs::create_dir_all(dir).expect("vault dir");
        let vault = Vault::load_or_create(&dir.join("vault.key")).expect("vault");
        Self {
            caller: Ok(Caller::Local(1000)),
            vault: Arc::new(vault),
        }
    }
}

/// Serve frames from `reader` until EOF or an I/O error on either side.
///
/// Frame-level problems get an error frame and leave the connection open.
pub(crate) fn handle(reader: impl BufRead, writer: impl Write, env: &Env, peer: &Peer) {
    serve(
        reader,
        writer,
        |id, request, out| match request {
            Request::ArtifactRead(req) => read_artifact(env, id, &req, out),
            request => match request.into_user() {
                Ok((asserted, cmd)) => answer_user(env, peer, asserted.as_ref(), &cmd),
                Err(request) => match requester(peer, &request) {
                    Ok(who) => respond_with_progress(env, who, id, *request, INTERVAL, out),
                    Err(e) => e.into_response(request.cmd(), vec![]),
                },
            },
        },
        |writer, since, filter, follow| {
            let ask = watch::Ask {
                since,
                filter: filter.clone(),
                follow,
            };
            watch::stream(writer, env, ask, watch::Limits::default());
        },
    );
}

/// [`handle`] with the engine behind `answer` and the event stream behind `watch`,
/// so tests can serve over fakes. `answer` may write frames for the request before its
/// response.
pub(super) fn serve<W: Write>(
    reader: impl BufRead,
    mut writer: W,
    answer: impl Fn(&str, Request, &mut dyn Write) -> Response,
    watch: impl Fn(&mut W, i64, &WatchFilter, bool),
) {
    let mut handshaken = false;
    for line in reader.lines() {
        let Ok(line) = line else { return };
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
            Ok(ClientFrame::Request { id, request }) => {
                let envelope = answer(&id, *request, &mut writer);
                ServerFrame::Response { id, envelope }
            }
        };
        if !send(&mut writer, &reply) {
            return;
        }
    }
}

/// Write one frame line and flush; false once the client is gone.
pub(super) fn send(writer: &mut (impl Write + ?Sized), frame: &ServerFrame) -> bool {
    writer.write_all(frame.to_line().as_bytes()).is_ok() && writer.flush().is_ok()
}

/// Stream an Artifact's bytes as bytes frames for request `id`, then answer.
fn read_artifact(env: &Env, id: &str, req: &ArtifactReadReq, out: &mut dyn Write) -> Response {
    match Moor::from_env(env) {
        Ok(moor) => moor.read_artifact(req, &mut |data| write_bytes(out, id, data)),
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

/// The identity a `start` or `start-needs` acts as: the one it asserts, checked against this
/// connection's socket. Other requests act as no one.
fn requester(peer: &Peer, request: &Request) -> Result<Option<Identity>, CommandError> {
    let asserted = match request {
        Request::Start(req) => req.identity.as_ref(),
        Request::StartNeeds(req) => req.identity.as_ref(),
        _ => return Ok(None),
    };
    peer.caller
        .as_ref()
        .map_err(Clone::clone)
        .and_then(|caller| caller.resolve(asserted))
        .map(Some)
        .map_err(|why| CommandError::new(ErrorKind::Refused, why))
}

/// Run `request` against a fresh engine acting as `who`, as `moor call` does.
///
/// `moor call` forces JSON output, so the run-output inheritance the CLI applies
/// to other invocations is a no-op there and is not applied here.
pub(crate) fn respond(env: &Env, who: Option<Identity>, request: Request) -> Response {
    let cmd = request.cmd();
    if matches!(request, Request::Version {}) {
        return Moor::version();
    }
    match Moor::from_env(env) {
        Ok(moor) => match who {
            Some(who) => moor.acting_as(who).call(request),
            None => moor.call(request),
        },
        Err(e) => e.into_response(cmd, vec![]),
    }
}

/// Run `run` against a fresh engine, passing the command's output to `out` as it runs.
pub(crate) fn respond_run(env: &Env, req: &RunReq, out: Sink<'_>) -> Response {
    match Moor::from_env(env) {
        Ok(moor) => moor.run_streamed(req, out),
        Err(e) => e.into_response("run", vec![]),
    }
}

#[cfg(test)]
#[path = "secret/tests.rs"]
mod secret;
#[cfg(test)]
#[path = "conn_tests.rs"]
mod tests;
