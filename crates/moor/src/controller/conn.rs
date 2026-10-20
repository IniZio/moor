//! One controller connection: reads client frames and answers each in order.

use std::io::{BufRead, Write};

use moor_core::controller::{
    parse_frame, write_bytes, ClientFrame, FrameErrorKind, ServerFrame, WatchFilter, PROTOCOL,
};
use moor_core::{ArtifactReadReq, Request, Response, RunReq};

use super::progress::{respond_with_progress, Sink, INTERVAL};
use super::watch;
use crate::env::Env;
use crate::Moor;

/// Serve frames from `reader` until EOF or an I/O error on either side.
///
/// Frame-level problems get an error frame and leave the connection open.
pub(crate) fn handle(reader: impl BufRead, writer: impl Write, env: &Env) {
    serve(
        reader,
        writer,
        |id, request, out| match request {
            Request::ArtifactRead(req) => read_artifact(env, id, &req, out),
            request => respond_with_progress(env, id, request, INTERVAL, out),
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

/// Run `request` against a fresh engine, as `moor call` does.
///
/// `moor call` forces JSON output, so the run-output inheritance the CLI applies
/// to other invocations is a no-op there and is not applied here.
pub(crate) fn respond(env: &Env, request: Request) -> Response {
    let cmd = request.cmd();
    if matches!(request, Request::Version {}) {
        return Moor::version();
    }
    match Moor::from_env(env) {
        Ok(moor) => moor.call(request),
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
