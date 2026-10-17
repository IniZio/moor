//! The controller's lease reaper: releases leases left behind by `moor close --force`.

use std::thread;
use std::time::Duration;

use super::{log, Sink};
use crate::env::Env;
use crate::Moor;

/// Gap between reaper rounds.
pub(crate) const EVERY: Duration = Duration::from_mins(10);

/// Reaps once at start, then every [`EVERY`], on a detached thread for the life of the process.
///
/// Each round wires fresh adapters so config changes apply; a failure is
/// logged to `sink` and the next round still runs.
pub(crate) fn spawn(env: &Env, sink: &Sink) {
    let env = env.clone();
    let sink = Sink::clone(sink);
    thread::spawn(move || {
        run_loop(thread::sleep, EVERY, &sink, None, || {
            match Moor::from_env(&env) {
                Ok(moor) => moor.reap_leases(),
                Err(e) => vec![e.error.to_string()],
            }
        });
    });
}

/// Ticks immediately, then sleeps `every` before each further tick; stops after `rounds` ticks.
///
/// `None` runs forever. Every line a tick returns goes to `sink`.
fn run_loop(
    mut sleep: impl FnMut(Duration),
    every: Duration,
    sink: &Sink,
    rounds: Option<usize>,
    mut tick: impl FnMut() -> Vec<String>,
) {
    let mut done = 0usize;
    while rounds.is_none_or(|n| done < n) {
        if done > 0 {
            sleep(every);
        }
        for line in tick() {
            log(sink, &format!("reap: {line}"));
        }
        done = done.saturating_add(1);
    }
}

#[cfg(test)]
mod tests;
