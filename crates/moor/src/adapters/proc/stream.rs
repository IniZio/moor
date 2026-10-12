//! Streamed children: output goes straight to moor's streams or a log file,
//! and an interrupt of moor stops the child's process group.

use std::fs::File;
use std::io;
use std::os::unix::process::ExitStatusExt;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use moor_core::seam::proc::{Output, ProcError, ProcSpec};
use signal_hook::consts::{SIGINT, SIGTERM};

use super::{feed, gone, spawn_retrying, stdin_for, terminate, SystemProc, POLL};

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
    /// an interrupt for the duration of the child.
    pub(super) fn stream_forwarding(
        &self,
        spec: &ProcSpec,
        out: &Output,
    ) -> Result<i32, ProcError> {
        let flag = Arc::new(AtomicBool::new(false));
        let ids = [SIGINT, SIGTERM]
            .into_iter()
            .map(|s| signal_hook::flag::register(s, Arc::clone(&flag)))
            .collect::<io::Result<Vec<_>>>()
            .map_err(ProcError::Spawn)?;
        let res = self.stream(spec, out, &flag);
        for id in ids {
            signal_hook::low_level::unregister(id);
        }
        res
    }

    /// Spawns the child, then waits for exit, the deadline or `interrupted`.
    pub(super) fn stream(
        &self,
        spec: &ProcSpec,
        out: &Output,
        interrupted: &AtomicBool,
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
        thread::spawn(move || tx.send(child.wait()).ok());
        let mut waited = Duration::ZERO;
        loop {
            match rx.recv_timeout(POLL) {
                Ok(st) => {
                    let st = st.map_err(ProcError::Spawn)?;
                    return st
                        .code()
                        .ok_or_else(|| ProcError::Signalled(st.signal().unwrap_or(0)));
                }
                Err(RecvTimeoutError::Disconnected) => return Err(ProcError::Spawn(gone())),
                Err(RecvTimeoutError::Timeout) => {}
            }
            if interrupted.load(Ordering::Relaxed) {
                terminate(pid, &rx, limit).map_err(ProcError::Spawn)?;
                return Err(ProcError::Interrupted);
            }
            waited += POLL;
            if waited >= limit {
                terminate(pid, &rx, limit).map_err(ProcError::Spawn)?;
                return Err(ProcError::Timeout {
                    tool: spec.tool,
                    after: limit,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests;
