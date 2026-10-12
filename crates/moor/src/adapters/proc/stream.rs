//! Streamed children: output goes to moor's streams or a log file (stderr
//! passes through moor to keep its tail), and an interrupt of moor stops the child's process group.

use std::fs::File;
use std::io::{self, Read, Write};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread;

use moor_core::seam::proc::{Output, ProcError, ProcSpec};
use signal_hook::consts::{SIGINT, SIGTERM};

use super::{
    failure, feed, gone, spawn_retrying, stdin_for, terminate, SystemProc, DRAIN_GRACE, POLL, TAIL,
};

/// Process-wide interrupt state, installed once and never unregistered.
struct Signals {
    /// Set by SIGINT or SIGTERM; cleared when the first stream starts.
    interrupted: Arc<AtomicBool>,
    /// True while no streamed child runs; the signals then act as default.
    idle: Arc<AtomicBool>,
    /// Streamed children currently running; held while `idle` changes with it.
    active: Mutex<usize>,
}

/// The one installation; `None` when a handler could not be registered.
static SIGNALS: OnceLock<Option<Signals>> = OnceLock::new();

impl Signals {
    /// Registers the handlers for SIGINT and SIGTERM.
    fn install() -> io::Result<Self> {
        let this = Self::new();
        for sig in [SIGINT, SIGTERM] {
            signal_hook::flag::register_conditional_default(sig, Arc::clone(&this.idle))?;
            signal_hook::flag::register(sig, Arc::clone(&this.interrupted))?;
        }
        Ok(this)
    }

    /// Idle state with no streams running.
    fn new() -> Self {
        Self {
            interrupted: Arc::new(AtomicBool::new(false)),
            idle: Arc::new(AtomicBool::new(true)),
            active: Mutex::new(0),
        }
    }

    /// Marks one stream running until the guard drops.
    fn enter(&self) -> Active<'_> {
        let mut n = self.active.lock().unwrap_or_else(PoisonError::into_inner);
        if *n == 0 {
            self.interrupted.store(false, Ordering::SeqCst);
            self.idle.store(false, Ordering::SeqCst);
        }
        *n += 1;
        drop(n);
        Active(self)
    }
}

/// Keeps the signals out of default mode while a stream runs.
struct Active<'a>(&'a Signals);

impl Drop for Active<'_> {
    fn drop(&mut self) {
        let mut n = self.0.active.lock().unwrap_or_else(PoisonError::into_inner);
        *n = n.saturating_sub(1);
        if *n == 0 {
            self.0.idle.store(true, Ordering::SeqCst);
        }
    }
}

/// Stops the interrupt watcher when dropped.
struct Done(Arc<AtomicBool>);

impl Drop for Done {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Where the child's stdout goes, and the writer that receives its stderr.
fn sinks(
    out: &Output,
    inherit_err: Box<dyn Write + Send>,
) -> io::Result<(Stdio, Box<dyn Write + Send>)> {
    match out {
        Output::Inherit => Ok((Stdio::inherit(), inherit_err)),
        Output::Log(path) => {
            let file = File::create(path)?;
            let twin = file.try_clone()?;
            Ok((file.into(), Box::new(twin)))
        }
    }
}

/// Copies `src` to `sink` as it arrives, keeping the last [`TAIL`] bytes in
/// `tail` as they come so a reader need not wait for EOF.
/// Keeps draining when the sink fails so the child never blocks on a full pipe.
fn tee(mut src: impl Read, mut sink: impl Write, tail: &Mutex<Vec<u8>>) {
    let mut buf = [0u8; 8192];
    let mut writable = true;
    loop {
        let n = match src.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let chunk = buf.get(..n).unwrap_or_default();
        if writable {
            writable = sink.write_all(chunk).and_then(|()| sink.flush()).is_ok();
        }
        let mut tail = tail.lock().unwrap_or_else(PoisonError::into_inner);
        tail.extend_from_slice(chunk);
        let excess = tail.len().saturating_sub(TAIL);
        tail.drain(..excess);
    }
}

impl SystemProc {
    /// Runs with output streamed, treating SIGINT and SIGTERM sent to moor as
    /// an interrupt for the duration of the child; outside it they keep their
    /// default action.
    pub(super) fn stream_forwarding(&self, spec: &ProcSpec, out: &Output) -> Result<(), ProcError> {
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
    ) -> Result<(), ProcError> {
        self.stream_to(spec, out, interrupted, Box::new(io::stderr()))
    }

    /// As [`Self::stream`], with `inherit_err` receiving stderr under [`Output::Inherit`].
    fn stream_to(
        &self,
        spec: &ProcSpec,
        out: &Output,
        interrupted: &Arc<AtomicBool>,
        inherit_err: Box<dyn Write + Send>,
    ) -> Result<(), ProcError> {
        let (stdin, payload) = stdin_for(&spec.stdin).map_err(ProcError::Spawn)?;
        let (stdout, err_sink) = sinks(out, inherit_err).map_err(ProcError::Spawn)?;
        let mut cmd = self.command(spec);
        cmd.stdin(stdin).stdout(stdout).stderr(Stdio::piped());
        let mut child = spawn_retrying(&mut cmd).map_err(ProcError::Spawn)?;
        drop(cmd);
        let w = child.stdin.take();
        let err_pipe = child.stderr.take();
        let tail = Arc::new(Mutex::new(Vec::new()));
        let (ttx, trx) = mpsc::channel();
        let tee_tail = Arc::clone(&tail);
        thread::spawn(move || {
            if let Some(p) = err_pipe {
                tee(p, err_sink, &tee_tail);
            }
            ttx.send(()).ok();
        });
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
        let done = Done(Arc::new(AtomicBool::new(false)));
        watch(Arc::clone(interrupted), Arc::clone(&done.0), etx);
        match erx.recv_timeout(limit) {
            Ok(Event::Exited) => {
                let st = rx
                    .recv()
                    .map_err(|_| ProcError::Spawn(gone()))?
                    .map_err(ProcError::Spawn);
                let st = st?;
                trx.recv_timeout(DRAIN_GRACE).ok();
                let tail = tail.lock().unwrap_or_else(PoisonError::into_inner).clone();
                if st.code() == Some(0) {
                    Ok(())
                } else {
                    Err(failure(st, &tail))
                }
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
        }
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
