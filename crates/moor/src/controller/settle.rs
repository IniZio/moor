//! The controller's periodic pass that keeps agent-state events current without any `show`.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use super::drain::Drain;
use super::{log, Sink};
use crate::env::Env;
use crate::Moor;

/// Settles every open Box each `every` on a detached thread, until `drain` begins.
///
/// Each tick wires fresh adapters so config and tool changes apply; a failure is
/// logged to `sink` and the next tick still runs.
pub(super) fn spawn(env: &Env, every: Duration, drain: &Arc<Drain>, sink: &Sink) {
    let env = env.clone();
    passes(every, drain, sink, move |draining| {
        match Moor::from_env(&env) {
            Ok(moor) => moor.settle_open(draining),
            Err(e) => vec![e.error.to_string()],
        }
    });
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
