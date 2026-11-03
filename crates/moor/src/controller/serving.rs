//! The controller's start-up sequence once its sockets are bound.

use std::ffi::OsString;
use std::os::unix::net::UnixStream;
use std::sync::Arc;

use super::drain::Drain;
use super::{log, settle, Running, Sink};
use crate::env::Env;
use crate::Moor;

/// Serves `running` until `wake` turns readable.
///
/// Interrupted starts are failed first, since requests read what that leaves. `reconcile` then
/// restarts the Box Proxies on a thread of its own, so requests are answered meanwhile; each
/// Proxy settles through the same lease its Box's start and close use.
pub(super) fn serve_started(
    mut running: Running,
    wake: &UnixStream,
    sink: &Sink,
    reconcile: impl FnOnce(&Env) -> Vec<String> + Send + 'static,
) -> u8 {
    log(sink, &format!("listening on {}", running.socket.display()));
    Moor::surface_override(&running.env).inspect(|line| log(sink, line));
    let interrupted = Moor::from_env(&running.env).map_or_else(
        |e| vec![e.error.to_string()],
        |moor| moor.fail_interrupted_starts(),
    );
    for line in interrupted {
        log(sink, &format!("start: {line}"));
    }
    let shim = std::env::current_exe().map_or_else(|_| "/proc/self/exe".into(), OsString::from);
    let mut fronts = running.supervise_fronts((shim, vec!["front-shim".into()]), sink);
    let drain = Arc::new(Drain::default());
    drain.end_fronts_with_drain(fronts.terminator(running.drain_timeout));
    settle::spawn(&running, &drain, sink);
    settle::spawn_oauth(
        &running.env,
        &running.vault,
        running.settle_every,
        &drain,
        sink,
    );
    reconcile_in_background(reconcile, &running.env, &drain, sink);
    running.serve_until(wake, &drain, sink);
    fronts.wait();
    0
}

/// Runs `reconcile` on its own thread; the drain waits for it, and none starts once it begins.
fn reconcile_in_background(
    reconcile: impl FnOnce(&Env) -> Vec<String> + Send + 'static,
    env: &Arc<Env>,
    drain: &Arc<Drain>,
    sink: &Sink,
) {
    let (env, drain, sink) = (Arc::clone(env), Arc::clone(drain), Arc::clone(sink));
    std::thread::spawn(move || {
        let Some(_held) = drain.hold() else { return };
        for line in reconcile(&env) {
            log(&sink, &format!("proxy: {line}"));
        }
    });
}
