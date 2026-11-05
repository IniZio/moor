//! The controller's periodic pass that keeps agent-state events current without any `show`.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use super::drain::Drain;
use super::{log, Running, Sink};
use crate::env::Env;
use crate::vault::Vault;
use crate::Moor;

/// Settles every open Box each `every` on a detached thread, until `drain` begins.
///
/// Each tick wires fresh adapters so config and tool changes apply; a failure is
/// logged to `sink` and the next tick still runs.
///
/// A Box that has been `setup_failed` for `close_after` is closed by the pass; `None` keeps them.
pub(super) fn spawn(running: &Running, drain: &Arc<Drain>, sink: &Sink) {
    let env = Env::clone(&running.env);
    let close_after = running.close_setup_failed_after;
    passes(
        running.settle_every,
        drain,
        sink,
        move |draining| match Moor::from_env(&env) {
            Ok(moor) => moor.settle_open_closing(draining, close_after),
            Err(e) => vec![e.error.to_string()],
        },
    );
}

/// Refreshes users' OAuth MCP access tokens near expiry each `every`, on its own thread so a
/// slow authorization server never delays a settle pass.
pub(super) fn spawn_oauth(
    env: &Env,
    vault: &Arc<Vault>,
    every: Duration,
    drain: &Arc<Drain>,
    sink: &Sink,
) {
    passes(
        every,
        drain,
        sink,
        Moor::oauth_refresher(env, Arc::clone(vault)),
    );
}

/// How often the controller looks for workflow runs to start, finish or expire.
const WORKFLOW_EVERY: Duration = Duration::from_secs(30);

/// Runs the workflow pass each [`WORKFLOW_EVERY`], on its own thread so a slow Box start or
/// a slow settle pass never delays it.
pub(super) fn spawn_workflows(env: &Env, drain: &Arc<Drain>, sink: &Sink) {
    passes(WORKFLOW_EVERY, drain, sink, Moor::workflow_pass(env));
}

/// Runs `pass` each `every` on a detached thread, logging each message it returns.
///
/// The drain waits for a pass already running. `pass` checks `draining` before each Box and
/// stops there; once the drain has begun no pass starts and the thread ends.
fn passes(
    every: Duration,
    drain: &Arc<Drain>,
    sink: &Sink,
    pass: impl Fn(&dyn Fn() -> bool) -> Vec<String> + Send + 'static,
) {
    let (drain, sink) = (Arc::clone(drain), Sink::clone(sink));
    thread::spawn(move || loop {
        thread::sleep(every);
        let Some(_held) = drain.hold() else { return };
        for msg in pass(&|| drain.begun()) {
            log(&sink, &format!("settle: {msg}"));
        }
    });
}

#[cfg(test)]
mod tests;
