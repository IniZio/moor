//! Progress frames for long verbs: the client sees liveness while a request runs.
//! A `run` also sends its command's output as bytes frames.

use std::io::{self, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use moor_core::controller::{write_bytes, ServerFrame};
use moor_core::{Request, Response};

use super::conn::{self, Acting};
use super::drain::Drain;
use super::hangup::Hangup;
use crate::env::{Cancel, Env};

/// Takes a running request's output piece by piece; an error means nobody reads it any more.
pub(super) type Sink<'a> = &'a mut (dyn FnMut(&[u8]) -> io::Result<()> + Send);

/// How often a running long verb reports that it is still alive.
pub(crate) const INTERVAL: Duration = Duration::from_secs(5);

/// Output pieces waiting for the connection thread; a slow client holds the run back at this
/// many, so the controller never buffers more of the output than this.
const PENDING: usize = 4;

/// How often a wait checks whether the controller began draining.
const TICK: Duration = Duration::from_millis(100);

/// Answer `request` acting as `acting`, writing its frames to `out` while a long verb runs.
///
/// A `run` is cancelled once `hangup` sees its client go away. A wait goes to
/// [`wait_unless_draining`] instead.
#[allow(
    clippy::too_many_arguments,
    reason = "the request, whom it acts as, and the connection it answers on"
)]
pub(crate) fn respond_with_progress(
    env: &Env,
    acting: Acting,
    hangup: Option<&Hangup>,
    id: &str,
    request: Request,
    interval: Duration,
    out: &mut dyn Write,
) -> Response {
    let cmd = request.cmd();
    match request {
        Request::Run(req) => {
            let cancel = Cancel::default();
            let env = env.clone().with_cancel(cancel.clone());
            let run = || {
                report_while(cmd, id, interval, out, |sink| {
                    conn::respond_run(&env, acting.grant, &req, sink)
                })
            };
            match hangup {
                Some(h) => h.cancels(&cancel, run),
                None => run(),
            }
        }
        request @ (Request::Start(_)
        | Request::Close(_)
        | Request::BoxMirror(_)
        | Request::BoxPut(_)) => report_while(cmd, id, interval, out, |_| {
            conn::respond(env, acting, request)
        }),
        request => conn::respond(env, acting, request),
    }
}

/// Answer a wait that `work` runs on its own thread, with a progress frame about each `interval`.
/// `None` once `drain` begins first: a wait changes nothing, so it is left to end with the
/// process, and one the drain began before is never started.
pub(super) fn wait_unless_draining(
    cmd: &str,
    id: &str,
    interval: Duration,
    out: &mut dyn Write,
    drain: &Drain,
    work: impl FnOnce() -> Response + Send + 'static,
) -> Option<Response> {
    if drain.begun() {
        return None;
    }
    let (done, answered) = mpsc::channel();
    let worker = thread::spawn(move || done.send(work()).ok());
    let every = (interval.as_millis() / TICK.as_millis()).max(1);
    let mut listening = true;
    let mut ticks = 0_u128;
    loop {
        match answered.recv_timeout(TICK) {
            Ok(response) => return Some(response),
            Err(RecvTimeoutError::Disconnected) => {
                return worker
                    .join()
                    .map_or_else(|p| std::panic::resume_unwind(p), |_| None)
            }
            Err(RecvTimeoutError::Timeout) if drain.begun() => return None,
            Err(RecvTimeoutError::Timeout) => {
                ticks += 1;
                if listening && ticks.is_multiple_of(every) {
                    let secs =
                        interval.as_secs() * u64::try_from(ticks / every).unwrap_or(u64::MAX);
                    let frame = ServerFrame::Progress {
                        id: id.to_owned(),
                        message: format!("{cmd}: still running ({secs}s)"),
                    };
                    listening = conn::send(out, &frame);
                }
            }
        }
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
