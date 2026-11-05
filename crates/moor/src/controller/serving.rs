//! The controller's start-up sequence once its sockets are bound.

use std::ffi::OsString;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::Duration;

use super::drain::Drain;
use super::{log, settle, Running, Sink};
use crate::env::Env;
use crate::Moor;
use moor_core::BoxId;

/// Serves `running` until `wake` turns readable.
///
/// Interrupted starts are failed first, since requests read what that leaves. `reconcile` then
/// restarts the Box Proxies on a thread of its own once `after` has passed, so requests are
/// answered meanwhile and a Proxy that rode out the restart has renewed its lease first.
pub(super) fn serve_started(
    mut running: Running,
    wake: &UnixStream,
    sink: &Sink,
    after: Duration,
    reconcile: impl FnOnce(&Env) -> Vec<String> + Send + 'static,
) -> u8 {
    log(sink, &format!("listening on {}", running.socket.display()));
    Moor::surface_override(&running.env).inspect(|line| log(sink, line));
    let (interrupted, claimed) = Moor::from_env(&running.env).map_or_else(
        |e| (vec![e.error.to_string()], Vec::new()),
        |moor| moor.fail_interrupted_starts(),
    );
    for line in interrupted {
        log(sink, &format!("start: {line}"));
    }
    resume_setups_in_background(claimed, &running.env, sink);
    for line in Moor::from_env(&running.env).map_or_else(
        |e| vec![e.error.to_string()],
        |moor| moor.resolve_interrupted_workflow_runs(),
    ) {
        log(sink, &format!("workflow: {line}"));
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
    settle::spawn_workflows(&running.env, &drain, sink);
    reconcile_in_background((after, reconcile), &running.env, &drain, sink);
    running.serve_until(wake, &drain, sink);
    fronts.wait();
    0
}

/// Follows again, on a thread of its own, the setups an earlier process left in `claimed`; the
/// drain does not wait for it, as the setup goes on in the Box and the next process follows it.
fn resume_setups_in_background(claimed: Vec<BoxId>, env: &Arc<Env>, sink: &Sink) {
    if claimed.is_empty() {
        return;
    }
    let (env, sink) = (Arc::clone(env), Arc::clone(sink));
    std::thread::spawn(move || {
        let lines = Moor::from_env(&env).map_or_else(
            |e| vec![e.error.to_string()],
            |moor| moor.resume_setups(&claimed),
        );
        for line in lines {
            log(&sink, &format!("setup: {line}"));
        }
    });
}

/// Runs `reconcile` on its own thread after `after`; the drain waits for it, and none starts
/// once it begins.
fn reconcile_in_background(
    (after, reconcile): (Duration, impl FnOnce(&Env) -> Vec<String> + Send + 'static),
    env: &Arc<Env>,
    drain: &Arc<Drain>,
    sink: &Sink,
) {
    let (env, drain, sink) = (Arc::clone(env), Arc::clone(drain), Arc::clone(sink));
    std::thread::spawn(move || {
        std::thread::sleep(after);
        let Some(_held) = drain.hold() else { return };
        for line in reconcile(&env) {
            log(&sink, &format!("proxy: {line}"));
        }
    });
}
