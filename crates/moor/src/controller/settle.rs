//! The controller's periodic pass that keeps agent-state events current without any `show`.

use std::thread;
use std::time::Duration;

use super::{log, Sink};
use crate::env::Env;
use crate::Moor;

/// Settles every open Task each `every` on a detached thread, for the life of the process.
///
/// Each tick wires fresh adapters so config and tool changes apply; a failure is
/// logged to `sink` and the next tick still runs.
pub(super) fn spawn(env: &Env, every: Duration, proof_cap: u64, sink: &Sink) {
    let env = env.clone();
    let sink = Sink::clone(sink);
    thread::spawn(move || loop {
        thread::sleep(every);
        tick(&env, proof_cap, &sink);
    });
}

/// One wired pass; every warning and failure goes to `sink`.
fn tick(env: &Env, proof_cap: u64, sink: &Sink) {
    match Moor::from_env(env) {
        Ok(moor) => {
            for msg in moor.settle_open(proof_cap) {
                log(sink, &format!("settle: {msg}"));
            }
        }
        Err(e) => log(sink, &format!("settle: {}", e.error)),
    }
}

#[cfg(test)]
mod tests;
