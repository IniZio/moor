//! Progress frames for long verbs: the client sees liveness while a request runs.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use moor_core::controller::ServerFrame;
use moor_core::{Request, Response};

use super::conn;
use crate::env::Env;

/// How often a running long verb reports that it is still alive.
pub(crate) const INTERVAL: Duration = Duration::from_secs(5);

/// Answer `request`, emitting a progress frame every `interval` while a long verb runs.
///
/// `emit` returns false once the client is gone; progress stops but the request
/// still finishes, so its side effects are not abandoned half-way.
pub(crate) fn respond_with_progress(
    env: &Env,
    id: &str,
    request: Request,
    interval: Duration,
    emit: &mut dyn FnMut(ServerFrame) -> bool,
) -> Response {
    let cmd = request.cmd();
    if matches!(
        request,
        Request::Start(_) | Request::Wait(_) | Request::Run(_)
    ) {
        report_while(cmd, id, interval, emit, || conn::respond(env, request))
    } else {
        conn::respond(env, request)
    }
}

/// Run `work` on a worker thread, emitting a progress frame per `interval` until it returns.
fn report_while<F>(
    cmd: &str,
    id: &str,
    interval: Duration,
    emit: &mut dyn FnMut(ServerFrame) -> bool,
    work: F,
) -> Response
where
    F: FnOnce() -> Response + Send,
{
    thread::scope(|scope| {
        let (done, finished) = mpsc::channel::<()>();
        let worker = scope.spawn(move || {
            let response = work();
            // Dropping the sender is the completion signal.
            drop(done);
            response
        });
        let mut listening = true;
        let mut ticks = 0_u64;
        while finished.recv_timeout(interval) == Err(RecvTimeoutError::Timeout) {
            ticks += 1;
            if listening {
                let secs = interval.as_secs() * ticks;
                listening = emit(ServerFrame::Progress {
                    id: id.to_owned(),
                    message: format!("{cmd}: still running ({secs}s)"),
                });
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
