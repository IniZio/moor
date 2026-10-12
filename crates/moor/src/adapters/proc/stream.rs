//! Streamed children: output goes straight to moor's streams or a log file,
//! and an interrupt of moor stops the child's process group.

use std::fs::File;
use std::io;
use std::os::unix::process::ExitStatusExt;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, OnceLock};
use std::thread;

use moor_core::seam::proc::{Output, ProcError, ProcSpec};
use signal_hook::consts::{SIGINT, SIGTERM};

use super::{feed, gone, spawn_retrying, stdin_for, terminate, SystemProc, POLL};

/// Process-wide interrupt state, installed once and never unregistered.
struct Signals {
    /// Set by SIGINT or SIGTERM; cleared when the first stream starts.
    interrupted: Arc<AtomicBool>,
    /// True while no streamed child runs; the signals then act as default.
    idle: Arc<AtomicBool>,
    /// Streamed children currently running.
    active: AtomicUsize,
}

/// The one installation; `None` when a handler could not be registered.
static SIGNALS: OnceLock<Option<Signals>> = OnceLock::new();

impl Signals {
    /// Registers the handlers for SIGINT and SIGTERM.
    fn install() -> io::Result<Self> {
        let interrupted = Arc::new(AtomicBool::new(false));
        let idle = Arc::new(AtomicBool::new(true));
        for sig in [SIGINT, SIGTERM] {
            signal_hook::flag::register_conditional_default(sig, Arc::clone(&idle))?;
            signal_hook::flag::register(sig, Arc::clone(&interrupted))?;
        }
        Ok(Self {
            interrupted,
            idle,
            active: AtomicUsize::new(0),
        })
    }

    /// Marks one stream running until the guard drops.
    fn enter(&self) -> Active<'_> {
        if self.active.fetch_add(1, Ordering::SeqCst) == 0 {
            self.interrupted.store(false, Ordering::SeqCst);
            self.idle.store(false, Ordering::SeqCst);
        }
        Active(self)
    }
}

/// Keeps the signals out of default mode while a stream runs.
struct Active<'a>(&'a Signals);

impl Drop for Active<'_> {
    fn drop(&mut self) {
        if self.0.active.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.0.idle.store(true, Ordering::SeqCst);
        }
    }
}

/// Stdout and stderr targets for the child.
fn sinks(out: &Output) -> io::Result<(Stdio, Stdio)> {
    match out {
        Output::Inherit => Ok((Stdio::inherit(), Stdio::inherit())),
        Output::Log(path) => {
            let file = File::create(path)?;
            let twin = file.try_clone()?;
            Ok((file.into(), twin.into()))
        }
    }
}

impl SystemProc {
    /// Runs with output streamed, treating SIGINT and SIGTERM sent to moor as
    /// an interrupt for the duration of the child; outside it they keep their
    /// default action.
    pub(super) fn stream_forwarding(
        &self,
        spec: &ProcSpec,
        out: &Output,
    ) -> Result<i32, ProcError> {
        let sig = SIGNALS
            .get_or_init(|| Signals::install().ok())
            .as_ref()
            .ok_or_else(|| ProcError::Spawn(io::Error::other("signal handlers unavailable")))?;
        let _active = sig.enter();
        self.stream(spec, out, &sig.interrupted)
    }

    /// Spawns the child, then waits for exit, the deadline or `interrupted`.
    pub(super) fn stream(
        &self,
        spec: &ProcSpec,
        out: &Output,
        interrupted: &Arc<AtomicBool>,
    ) -> Result<i32, ProcError> {
        let (stdin, payload) = stdin_for(&spec.stdin).map_err(ProcError::Spawn)?;
        let (stdout, stderr) = sinks(out).map_err(ProcError::Spawn)?;
        let mut cmd = self.command(spec);
        cmd.stdin(stdin).stdout(stdout).stderr(stderr);
        let mut child = spawn_retrying(&mut cmd).map_err(ProcError::Spawn)?;
        drop(cmd);
        let w = child.stdin.take();
        let payload = payload.map(<[u8]>::to_vec);
        thread::spawn(move || feed(w, payload.as_deref()));
        let limit = spec.deadline.0;
        let pid = child.id();
        let (tx, rx) = mpsc::channel();
        let (etx, erx) = mpsc::channel();
        let exited = etx.clone();
        thread::spawn(move || {
            tx.send(child.wait()).ok();
            exited.send(Event::Exited).ok();
        });
        let done = Arc::new(AtomicBool::new(false));
        watch(Arc::clone(interrupted), Arc::clone(&done), etx);
        let res = match erx.recv_timeout(limit) {
            Ok(Event::Exited) => {
                let st = rx
                    .recv()
                    .map_err(|_| ProcError::Spawn(gone()))?
                    .map_err(ProcError::Spawn);
                st.and_then(|st| {
                    st.code()
                        .ok_or_else(|| ProcError::Signalled(st.signal().unwrap_or(0)))
                })
            }
            Ok(Event::Interrupted) => terminate(pid, &rx, limit)
                .map_err(ProcError::Spawn)
                .and(Err(ProcError::Interrupted)),
            Err(RecvTimeoutError::Timeout) => terminate(pid, &rx, limit)
                .map_err(ProcError::Spawn)
                .and(Err(ProcError::Timeout {
                    tool: spec.tool,
                    after: limit,
                })),
            Err(RecvTimeoutError::Disconnected) => Err(ProcError::Spawn(gone())),
        };
        done.store(true, Ordering::SeqCst);
        res
    }
}

/// What ended the wait.
enum Event {
    /// The child exited.
    Exited,
    /// The interrupt flag was set.
    Interrupted,
}

/// Reports [`Event::Interrupted`] once `flag` is set, until `done`.
fn watch(flag: Arc<AtomicBool>, done: Arc<AtomicBool>, tx: mpsc::Sender<Event>) {
    thread::spawn(move || {
        while !done.load(Ordering::SeqCst) {
            if flag.load(Ordering::SeqCst) {
                tx.send(Event::Interrupted).ok();
                return;
            }
            thread::sleep(POLL);
        }
    });
}

#[cfg(test)]
mod tests;
