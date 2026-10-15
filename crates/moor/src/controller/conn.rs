//! One controller connection: reads client frames and answers each in order.

use std::io::{BufRead, Write};

use moor_core::controller::{parse_frame, ClientFrame, FrameErrorKind, ServerFrame, PROTOCOL};
use moor_core::{Request, Response};

use crate::env::Env;
use crate::Moor;

/// Serve frames from `reader` until EOF or an I/O error on either side.
///
/// Frame-level problems get an error frame and leave the connection open.
pub(crate) fn handle(reader: impl BufRead, writer: impl Write, env: &Env) {
    serve(reader, writer, |request| respond(env, request));
}

/// [`handle`] with the engine behind `answer`, so tests can serve over fakes.
fn serve(reader: impl BufRead, mut writer: impl Write, answer: impl Fn(Request) -> Response) {
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
            Ok(ClientFrame::Request { id, .. }) if !handshaken => ServerFrame::Error {
                message: format!("request {id} arrived before hello"),
                id: Some(id),
                kind: FrameErrorKind::HelloRequired,
            },
            Ok(ClientFrame::Request { id, request }) => ServerFrame::Response {
                id,
                envelope: answer(request),
            },
        };
        if writer.write_all(reply.to_line().as_bytes()).is_err() || writer.flush().is_err() {
            return;
        }
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

#[cfg(test)]
#[path = "secret/tests.rs"]
mod secret;
#[cfg(test)]
#[path = "conn_tests.rs"]
mod tests;
