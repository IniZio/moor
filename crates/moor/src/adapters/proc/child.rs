//! Long-lived children: the caller reads one ready line from stdout and the
//! child, with every process in its group, stops when the handle drops.

use std::io::{self, BufRead, BufReader, Read};
use std::os::unix::process::ExitStatusExt;
use std::process::{ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use moor_core::seam::proc::{ChildHandle, Deadline, FirstLineError, ProcError, ProcSpec, Tool};
use rustix::process::Signal;
use zeroize::Zeroizing;

use super::{
    feed, gone, signal_group, spawn_retrying, stdin_for, terminate, SystemProc, Waited,
    DRAIN_GRACE, GRACE, TAIL,
};

/// What the stdout reader found first: a full line, or the end of output.
type FirstRead = io::Result<Option<String>>;

/// A running child; dropping it stops the child's whole process group.
struct Child {
    /// Tool run, named in timeout errors.
    tool: Tool,
    /// Leader pid, equal to the process group id.
    pid: u32,
    /// Exit status of the leader, sent once by the waiter thread.
    exit: Receiver<Waited>,
    /// First stdout line or the reason there is none.
    line: Receiver<FirstRead>,
    /// Fires when the stderr reader reaches end of stream.
    err_done: Receiver<()>,
    /// Last bytes of stderr.
    tail: Arc<Mutex<Vec<u8>>>,
    /// Ready line once read.
    cached: Option<String>,
    /// True once the exit status has been taken from `exit`.
    reaped: bool,
}

impl SystemProc {
    /// Spawns a child that outlives the call; stdout and stderr stay drained.
    pub(super) fn spawn_long_lived(
        &self,
        spec: &ProcSpec,
    ) -> Result<Box<dyn ChildHandle>, ProcError> {
        let (stdin, payload) = stdin_for(&spec.stdin).map_err(ProcError::Spawn)?;
        let mut cmd = self.command(spec);
        cmd.stdin(stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = spawn_retrying(&mut cmd).map_err(ProcError::Spawn)?;
        let (w, o, e) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let payload = payload.map(|b| Zeroizing::new(b.to_vec()));
        thread::spawn(move || feed(w, payload.as_ref().map(|p| p.as_slice())));
        let (line_tx, line) = mpsc::channel();
        if let Some(o) = o {
            thread::spawn(move || read_stdout(o, &line_tx));
        }
        let tail = Arc::new(Mutex::new(Vec::new()));
        let (done_tx, err_done) = mpsc::channel();
        if let Some(e) = e {
            let tail = Arc::clone(&tail);
            thread::spawn(move || keep_tail(e, &tail, &done_tx));
        }
        let pid = child.id();
        let (tx, exit) = mpsc::channel();
        thread::spawn(move || tx.send(child.wait()).ok());
        Ok(Box::new(Child {
            tool: spec.tool,
            pid,
            exit,
            line,
            err_done,
            tail,
            cached: None,
            reaped: false,
        }))
    }
}

/// Sends the first full line, then discards the rest so the child never
/// blocks on a full pipe. A partial line at end of stream is no line.
fn read_stdout<R: Read>(pipe: R, tx: &Sender<FirstRead>) {
    let mut reader = BufReader::new(pipe);
    let mut buf = String::new();
    let first = match reader.read_line(&mut buf) {
        Ok(_) if buf.ends_with('\n') => Ok(Some(buf)),
        Ok(_) => Ok(None),
        Err(e) => Err(e),
    };
    let sent = matches!(first, Ok(Some(_)));
    tx.send(first).ok();
    if sent {
        io::copy(&mut reader, &mut io::sink()).ok();
    }
}

/// Keeps the last [`TAIL`] bytes of stderr until end of stream.
fn keep_tail<R: Read>(mut pipe: R, tail: &Mutex<Vec<u8>>, done: &Sender<()>) {
    let mut chunk = [0_u8; 4096];
    while let Ok(n @ 1..) = pipe.read(&mut chunk) {
        let mut t = tail.lock().unwrap_or_else(PoisonError::into_inner);
        t.extend_from_slice(chunk.get(..n).unwrap_or_default());
        let extra = t.len().saturating_sub(TAIL);
        t.drain(..extra);
    }
    done.send(()).ok();
}

impl Child {
    /// Exit status and stderr tail for a child that ended before a full line.
    fn exited(&mut self) -> FirstLineError {
        let status: Result<ExitStatus, io::Error> = match self.exit.recv_timeout(GRACE) {
            Ok(st) => {
                self.reaped = true;
                st
            }
            Err(RecvTimeoutError::Timeout) => {
                return FirstLineError::Io(io::Error::other("child closed stdout but kept running"))
            }
            Err(RecvTimeoutError::Disconnected) => Err(gone()),
        };
        self.err_done.recv_timeout(DRAIN_GRACE).ok();
        let stderr_tail = {
            let tail = self.tail.lock().unwrap_or_else(PoisonError::into_inner);
            String::from_utf8_lossy(&tail).into_owned()
        };
        match status {
            Ok(st) => FirstLineError::Exited {
                code: st.code(),
                signal: st.signal(),
                stderr_tail,
            },
            Err(e) => FirstLineError::Io(e),
        }
    }
}

impl ChildHandle for Child {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn first_line(&mut self, deadline: Deadline) -> Result<String, FirstLineError> {
        if let Some(line) = &self.cached {
            return Ok(line.clone());
        }
        match self.line.recv_timeout(deadline.0) {
            Ok(Ok(Some(raw))) => {
                let line = raw.trim_end_matches(['\n', '\r']).to_owned();
                self.cached = Some(line.clone());
                Ok(line)
            }
            Ok(Ok(None)) | Err(RecvTimeoutError::Disconnected) => Err(self.exited()),
            Ok(Err(e)) => Err(FirstLineError::Io(e)),
            Err(RecvTimeoutError::Timeout) => Err(FirstLineError::Timeout {
                tool: self.tool,
                after: deadline.0,
            }),
        }
    }
}

impl Drop for Child {
    /// Stops the whole group; the kill also reaches grandchildren that outlive
    /// an already exited leader.
    fn drop(&mut self) {
        if self.reaped {
            signal_group(self.pid, Signal::KILL);
        } else {
            terminate(self.pid, &self.exit, GRACE).ok();
        }
    }
}

#[cfg(test)]
mod tests;
