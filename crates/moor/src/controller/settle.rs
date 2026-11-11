//! The controller's periodic pass that keeps agent-state events current without any `show`.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
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
/// Also each [`RECONCILE_EVERY`], lists every provider holding open Boxes and marks those it no
/// longer lists, so a lost VM is noticed without anyone reading the Box.
///
/// A Box that has been `setup_failed` for `close_after` is closed by the pass; `None` keeps them.
pub(super) fn spawn(running: &Running, drain: &Arc<Drain>, sink: &Sink) {
    let env = Env::clone(&running.env);
    let close_after = running.close_setup_failed_after;
    let reconcile = Every::new(RECONCILE_EVERY, running.settle_every);
    passes(
        running.settle_every,
        drain,
        sink,
        move |draining| match Moor::from_env(&env) {
            Ok(moor) => {
                let mut out = moor.settle_open_closing(draining, close_after);
                if reconcile.due() {
                    out.extend(moor.reconcile_open(draining));
                }
                out
            }
            Err(e) => vec![e.error.to_string()],
        },
    );
}

/// How often the settle pass lists providers to find Boxes whose VM is gone. Each run costs one
/// listing per provider with open Boxes, and a lost VM is noticed within this plus one settle
/// tick, so a minute keeps listings rare against the 5 s settle default.
const RECONCILE_EVERY: Duration = Duration::from_secs(60);

/// Counts settle ticks to say which are due for a job that runs once per `period`.
struct Every {
    /// Ticks between due calls, at least 1.
    ticks: u64,
    /// Calls so far.
    seen: AtomicU64,
}

impl Every {
    /// A gate firing on the first call, then once per `period` of `tick`-long calls.
    fn new(period: Duration, tick: Duration) -> Self {
        let ticks = period.as_millis().div_ceil(tick.as_millis().max(1));
        Self {
            ticks: u64::try_from(ticks).unwrap_or(u64::MAX).max(1),
            seen: AtomicU64::new(0),
        }
    }

    /// Whether this call is due.
    fn due(&self) -> bool {
        self.seen
            .fetch_add(1, Ordering::Relaxed)
            .is_multiple_of(self.ticks)
    }
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

/// Runs the inbox pass each `every`, on its own thread, since waking a Box or bringing one back
/// on new hands can take minutes and must not hold up settling the others.
pub(super) fn spawn_inbox(env: &Env, every: Duration, drain: &Arc<Drain>, sink: &Sink) {
    passes(every, drain, sink, Moor::inbox_pass(env));
}

/// How often the controller says how many watch streams are open, when the count changed.
const WATCH_COUNT_EVERY: Duration = Duration::from_secs(60);

/// Logs the number of open watch streams each [`WATCH_COUNT_EVERY`] when it changed since the
/// last line, so streams that pile up show in the log.
pub(super) fn spawn_watch_count(drain: &Arc<Drain>, sink: &Sink) {
    let said = AtomicUsize::new(0);
    passes(WATCH_COUNT_EVERY, drain, sink, move |_| {
        let live = super::watch::live();
        let before = said.swap(live, Ordering::Relaxed);
        if live == before {
            return Vec::new();
        }
        vec![format!(
            "{live} watch streams open, {before} a minute before"
        )]
    });
}

/// Runs the wanted pass each `every`, on its own thread, since a close that waited for room wakes
/// its Box first and must not hold up the inbox or settling the others.
pub(super) fn spawn_wanted(env: &Env, every: Duration, drain: &Arc<Drain>, sink: &Sink) {
    passes(every, drain, sink, Moor::wanted_pass(env));
}

/// Runs the promotion pass each `every`, on its own thread, since setting up a promoted Box can
/// take many minutes and must not hold up a close or the other passes.
pub(super) fn spawn_promote(env: &Env, every: Duration, drain: &Arc<Drain>, sink: &Sink) {
    passes(every, drain, sink, Moor::promote_pass(env));
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
        for msg in Moor::checked(|| pass(&|| drain.begun())) {
            log(&sink, &format!("settle: {msg}"));
        }
    });
}

#[cfg(test)]
mod tests;
