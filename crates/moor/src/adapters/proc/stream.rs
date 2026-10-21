//! Streamed children: output goes to moor's streams or a log file (stderr
//! passes through moor to keep its tail), and an interrupt of moor stops the child's process group.

use std::fs::File;
use std::io::{self, Read, Write};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread;

use moor_core::seam::proc::{Output, ProcError, ProcSpec, StderrGate};
use signal_hook::consts::{SIGINT, SIGTERM};

use super::grace::{terminate_with, Abort};
use super::{failure, feed, gone, stdin_for, SystemProc, DRAIN_GRACE, POLL, TAIL};
use crate::env::Cancel;

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

/// A writer shared between the stderr copier and the wait notice.
#[derive(Clone)]
struct Shared(Arc<Mutex<Box<dyn Write + Send>>>);

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .flush()
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

/// Longest line the gate buffers; a longer one is cut here and handled as two.
const LINE_CAP: usize = 8 * 1024;

/// Where a gated stream stands relative to its marker lines.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The begin marker has not appeared.
    Before,
    /// Between the markers: lines are forwarded.
    Open,
    /// The end marker has passed.
    After,
}

/// Line filter that forwards only what lies between two marker lines.
struct Gate {
    /// The marker lines, matched whole.
    markers: StderrGate,
    /// Position relative to the markers.
    phase: Phase,
    /// Bytes of the line being read.
    line: Vec<u8>,
}

impl Gate {
    /// A closed gate over `markers`.
    const fn new(markers: StderrGate) -> Self {
        Self {
            markers,
            phase: Phase::Before,
            line: Vec::new(),
        }
    }

    /// Takes the next bytes and returns the ones to forward.
    fn feed(&mut self, chunk: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for &b in chunk {
            self.line.push(b);
            if b == b'\n' || self.line.len() >= LINE_CAP {
                self.finish_line(&mut out);
            }
        }
        out
    }

    /// Handles a final line that never got its newline; call at EOF.
    fn finish(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        if !self.line.is_empty() {
            self.finish_line(&mut out);
        }
        out
    }

    /// Classifies the buffered line, appending it to `out` when it is forwarded.
    fn finish_line(&mut self, out: &mut Vec<u8>) {
        let line = std::mem::take(&mut self.line);
        let text = line.strip_suffix(b"\n").unwrap_or(&line);
        let text = text.strip_suffix(b"\r").unwrap_or(text);
        match self.phase {
            Phase::Before if text == self.markers.begin.as_bytes() => self.phase = Phase::Open,
            Phase::Open if text == self.markers.end.as_bytes() => self.phase = Phase::After,
            Phase::Open => out.extend_from_slice(&line),
            Phase::Before | Phase::After => {}
        }
    }
}

/// Writes and flushes `bytes`, returning whether the sink still works.
fn emit(sink: &mut impl Write, bytes: &[u8]) -> bool {
    bytes.is_empty() || sink.write_all(bytes).and_then(|()| sink.flush()).is_ok()
}

/// Copies `src` to `sink` as it arrives, keeping the last [`TAIL`] bytes in
/// `tail` as they come so a reader need not wait for EOF.
/// With a `gate`, the sink gets only the lines between its markers, but the
/// tail still keeps every byte.
/// Keeps draining when the sink fails so the child never blocks on a full pipe.
fn tee(mut src: impl Read, mut sink: impl Write, tail: &Mutex<Vec<u8>>, mut gate: Option<Gate>) {
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
            writable = match gate.as_mut() {
                Some(g) => emit(&mut sink, &g.feed(chunk)),
                None => emit(&mut sink, chunk),
            };
        }
        let mut tail = tail.lock().unwrap_or_else(PoisonError::into_inner);
        tail.extend_from_slice(chunk);
        let excess = tail.len().saturating_sub(TAIL);
        tail.drain(..excess);
    }
    if let (true, Some(g)) = (writable, gate.as_mut()) {
        emit(&mut sink, &g.finish());
    }
}

impl SystemProc {
    /// Runs with output streamed, treating SIGINT and SIGTERM sent to moor, or a cancel of
    /// the request, as an interrupt for the duration of the child; outside it the signals
    /// keep their default action.
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
        let mut notice = Shared(Arc::new(Mutex::new(inherit_err)));
        let (stdout, err_sink) = sinks(out, Box::new(notice.clone())).map_err(ProcError::Spawn)?;
        let mut cmd = self.command(spec);
        cmd.stdin(stdin).stdout(stdout).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(ProcError::Spawn)?;
        drop(cmd);
        let w = child.stdin.take();
        let err_pipe = child.stderr.take();
        let tail = Arc::new(Mutex::new(Vec::new()));
        let (ttx, trx) = mpsc::channel();
        let tee_tail = Arc::clone(&tail);
        let gate = spec.stderr_gate.clone().map(Gate::new);
        thread::spawn(move || {
            if let Some(p) = err_pipe {
                tee(p, err_sink, &tee_tail, gate);
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
        let stops = (Arc::clone(interrupted), self.cancel.clone());
        watch(stops, Arc::clone(&done.0), etx);
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
            Ok(Event::Interrupted) => {
                interrupted.store(false, Ordering::SeqCst);
                stop(pid, &rx, spec, &mut notice, interrupted).and(Err(ProcError::Interrupted))
            }
            Err(RecvTimeoutError::Timeout) => {
                stop(pid, &rx, spec, &mut notice, interrupted).and(Err(ProcError::Timeout {
                    tool: spec.tool,
                    after: limit,
                }))
            }
            Err(RecvTimeoutError::Disconnected) => Err(ProcError::Spawn(gone())),
        }
    }
}

/// Terminates the child, letting another interrupt cut a crabbox wait short.
fn stop(
    pid: u32,
    rx: &mpsc::Receiver<super::Waited>,
    spec: &ProcSpec,
    notice: &mut Shared,
    flag: &AtomicBool,
) -> Result<(), ProcError> {
    let abort = Abort { flag, notice };
    terminate_with(pid, rx, spec.tool, spec.deadline.0, Some(abort)).map_err(ProcError::Spawn)
}

/// What ended the wait.
enum Event {
    /// The child exited.
    Exited,
    /// The interrupt flag was set.
    Interrupted,
}

/// Reports [`Event::Interrupted`] once moor is interrupted or the request is cancelled, until `done`.
fn watch(
    (flag, cancel): (Arc<AtomicBool>, Cancel),
    done: Arc<AtomicBool>,
    tx: mpsc::Sender<Event>,
) {
    thread::spawn(move || {
        while !done.load(Ordering::SeqCst) {
            if flag.load(Ordering::SeqCst) || cancel.is_cancelled() {
                tx.send(Event::Interrupted).ok();
                return;
            }
            thread::sleep(POLL);
        }
    });
}

#[cfg(test)]
mod tests;
