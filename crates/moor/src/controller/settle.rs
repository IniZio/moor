//! The controller's periodic pass that keeps agent-state events current without any `show`.

use std::thread;
use std::time::Duration;

use super::{log, Sink};
use crate::env::Env;
use crate::Moor;

/// Settles every open Task each `every` on a detached thread, for the life of the process.
///
/// Each tick wires fresh adapters so config and tool changes apply; a failure is
/// logged to `sink` and the next tick still runs. The thread owns the proof
/// backoff, since `Moor` is rebuilt every tick.
pub(super) fn spawn(env: &Env, every: Duration, proof_cap: u64, sink: &Sink) {
    let env = env.clone();
    let sink = Sink::clone(sink);
    thread::spawn(move || {
        let backoff = Moor::settle_backoff();
        loop {
            thread::sleep(every);
            tick(&env, |moor| moor.settle_open(proof_cap, &backoff), &sink);
        }
    });
}

/// One wired pass; every warning and failure goes to `sink`.
fn tick(env: &Env, settle: impl FnOnce(&Moor) -> Vec<String>, sink: &Sink) {
    match Moor::from_env(env) {
        Ok(moor) => {
            for msg in settle(&moor) {
                log(sink, &format!("settle: {msg}"));
            }
        }
        Err(e) => log(sink, &format!("settle: {}", e.error)),
    }
}

#[cfg(test)]
mod tests;
