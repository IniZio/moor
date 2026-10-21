//! Progress frames for long verbs: the client sees liveness while a request runs.
//! A `run` also sends its command's output as bytes frames.

use std::io::{self, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use moor_core::controller::{write_bytes, ServerFrame};
use moor_core::{Request, Response};

use super::conn;
use crate::env::Env;

/// Takes a running request's output piece by piece; an error means nobody reads it any more.
pub(super) type Sink<'a> = &'a mut (dyn FnMut(&[u8]) -> io::Result<()> + Send);

/// How often a running long verb reports that it is still alive.
pub(crate) const INTERVAL: Duration = Duration::from_secs(5);

/// Output pieces waiting for the connection thread; a slow client holds the run back at this
/// many, so the controller never buffers more of the output than this.
const PENDING: usize = 4;

/// Answer `request`, writing its frames to `out` while a long verb runs.
pub(crate) fn respond_with_progress(
    env: &Env,
    id: &str,
    request: Request,
    interval: Duration,
    out: &mut dyn Write,
) -> Response {
    let cmd = request.cmd();
    match request {
        Request::Run(req) => report_while(cmd, id, interval, out, |sink| {
            conn::respond_run(env, &req, sink)
        }),
        request @ (Request::Start(_)
        | Request::Wait(_)
        | Request::WaitEvents(_)
        | Request::Close(_)) => {
            report_while(cmd, id, interval, out, |_| conn::respond(env, request))
        }
        request => conn::respond(env, request),
    }
}

/// Run `work` on a worker thread until it returns. Each piece `work` passes to its sink goes
/// to `out` as a bytes frame. Each `interval` without one sends a progress frame, which counts
/// only those quiet intervals.
///
/// A failed write means the client is gone: frames stop, but pieces are still taken, so
/// `work` finishes and its side effects are not abandoned half-way.
pub(super) fn report_while<F>(
    cmd: &str,
    id: &str,
    interval: Duration,
    out: &mut dyn Write,
    work: F,
) -> Response
where
    F: FnOnce(Sink<'_>) -> Response + Send,
{
    thread::scope(|scope| {
        let (pieces, taken) = mpsc::sync_channel::<Vec<u8>>(PENDING);
        // The sender drops when `work` returns, which ends the loop below.
        let worker = scope.spawn(move || {
            work(&mut |data: &[u8]| {
                pieces
                    .send(data.to_vec())
                    .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
            })
        });
        let mut listening = true;
        let mut quiet = 0_u64;
        loop {
            match taken.recv_timeout(interval) {
                Ok(data) if listening => listening = write_bytes(out, id, &data).is_ok(),
                Err(RecvTimeoutError::Timeout) if listening => {
                    quiet += 1;
                    let secs = interval.as_secs() * quiet;
                    let frame = ServerFrame::Progress {
                        id: id.to_owned(),
                        message: format!("{cmd}: still running ({secs}s)"),
                    };
                    listening = conn::send(out, &frame);
                }
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        worker
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

#[cfg(test)]
#[path = "progress_tests.rs"]
mod tests;
