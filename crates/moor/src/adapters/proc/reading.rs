//! `Proc::run_reading`: stdout goes to the caller as it arrives, never held whole.

use std::io::{self, Read};
use std::process::{ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;

use moor_core::seam::proc::{ProcError, ProcSpec};
use zeroize::Zeroizing;

use super::grace::terminate;
use super::{drain, failure, feed, gone, stdin_for, take, SystemProc, Waited, DRAIN_GRACE};

/// What the watchdog reports once the child is reaped.
type Reaped = Result<ExitStatus, ProcError>;

impl SystemProc {
    /// Runs to completion, handing stdout to `read` on this thread while a
    /// watchdog enforces the deadline by killing the child's group.
    pub(super) fn reading(
        &self,
        spec: &ProcSpec,
        read: &mut dyn FnMut(&mut dyn Read),
    ) -> Result<(), ProcError> {
        let (stdin, payload) = stdin_for(&spec.stdin).map_err(ProcError::Spawn)?;
        let mut cmd = self.command(spec);
        cmd.stdin(stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(ProcError::Spawn)?;
        let (w, o, e) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let payload = payload.map(|b| Zeroizing::new(b.to_vec()));
        thread::spawn(move || feed(w, payload.as_ref().map(|p| p.as_slice())));
        let (etx, erx) = mpsc::channel();
        let err = drain(e, etx);
        let (pid, limit, tool) = (child.id(), spec.deadline.0, spec.tool);
        let (tx, rx) = mpsc::channel::<Waited>();
        thread::spawn(move || tx.send(child.wait()).ok());
        let (rtx, reaped) = mpsc::channel::<Reaped>();
        thread::spawn(move || {
            let r = match rx.recv_timeout(limit) {
                Ok(st) => st.map_err(ProcError::Spawn),
                Err(RecvTimeoutError::Timeout) => terminate(pid, &rx, tool, limit)
                    .map_err(ProcError::Spawn)
                    .and(Err(ProcError::Timeout { tool, after: limit })),
                Err(RecvTimeoutError::Disconnected) => Err(ProcError::Spawn(gone())),
            };
            rtx.send(r).ok();
        });
        if let Some(mut out) = o {
            read(&mut out);
            drop(io::copy(&mut out, &mut io::sink()));
        }
        let status = reaped.recv().map_err(|_| ProcError::Spawn(gone()))??;
        erx.recv_timeout(DRAIN_GRACE).ok();
        if status.code() == Some(0) {
            return Ok(());
        }
        Err(failure(status, &take(&err).buf))
    }
}

#[cfg(test)]
mod tests;
