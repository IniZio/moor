//! A child whose stdin, stdout and stderr stream to its caller as raw bytes, without a terminal.

use std::io::{self, Read, Write};
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::Duration;

/// How often a piped child checks whether its caller went away.
const STOP_POLL: Duration = Duration::from_millis(100);

/// How long to wait between checks for a child that closed its output but has not exited.
const REAP_POLL: Duration = Duration::from_millis(2);

/// The caller's ends of a command whose stdio streams both ways.
pub struct Pipes<'a> {
    /// Bytes for the command's stdin; its end closes the command's stdin. Owned, because a read
    /// that blocks after the command ended must not hold the caller.
    pub stdin: Box<dyn Read + Send>,
    /// Where the command's stdout goes.
    pub stdout: &'a mut (dyn Write + Send),
    /// Where the command's stderr goes.
    pub stderr: &'a mut (dyn Write + Send),
    /// True once the caller is gone and the command must be killed.
    pub stop: &'a (dyn Fn() -> bool + Sync),
}

/// How a piped child ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Piped {
    /// Its exit code; `None` when a signal ended it.
    pub code: Option<i32>,
    /// It was killed because the caller went away or stopped taking its output.
    pub stopped: bool,
}

/// Streams `child`'s stdio through `pipes` until it exits, writing `preamble` to its stdin
/// before the caller's bytes. `stop`, or a failed write of its output, kills it.
///
/// # Errors
/// The child was spawned without piped stdio, or waiting on it failed.
pub fn pump(mut child: Child, preamble: Vec<u8>, pipes: Pipes<'_>) -> io::Result<Piped> {
    let (Some(mut input), Some(mut out), Some(mut err)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(io::Error::other("the command's stdio is not piped"));
    };
    let Pipes {
        mut stdin,
        stdout,
        stderr,
        stop,
    } = pipes;
    thread::spawn(move || {
        if input.write_all(&preamble).is_ok() {
            let _ = io::copy(&mut stdin, &mut input);
        }
    });
    let child = Mutex::new(child);
    let gone = AtomicBool::new(false);
    let stopped = AtomicBool::new(false);
    let (ended, wait) = mpsc::channel::<()>();
    let status = thread::scope(|s| {
        let (gone, stopped, child) = (&gone, &stopped, &child);
        s.spawn(move || {
            while wait.recv_timeout(STOP_POLL) == Err(RecvTimeoutError::Timeout) {
                if stop() || gone.load(Ordering::SeqCst) {
                    stopped.store(true, Ordering::SeqCst);
                    let _ = lock(child).kill();
                    return;
                }
            }
        });
        let copies = [
            s.spawn(|| copy_out(&mut out, stdout, gone)),
            s.spawn(|| copy_out(&mut err, stderr, gone)),
        ];
        for copy in copies {
            let _ = copy.join();
        }
        let status = reap(child);
        drop(ended);
        status
    })?;
    Ok(Piped {
        code: status.code(),
        stopped: stopped.load(Ordering::SeqCst),
    })
}

/// The child's exit status once it exits; the lock is let go between checks so it can be killed.
fn reap(child: &Mutex<Child>) -> io::Result<std::process::ExitStatus> {
    loop {
        let exited = lock(child).try_wait()?;
        if let Some(status) = exited {
            return Ok(status);
        }
        thread::sleep(REAP_POLL);
    }
}

/// Copies `from` to `to` until `from` ends; a failed write marks the caller `gone`.
fn copy_out(from: &mut impl Read, to: &mut (dyn Write + Send), gone: &AtomicBool) {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = match from.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return,
        };
        let chunk = buf.get(..n).unwrap_or_default();
        if to.write_all(chunk).and_then(|()| to.flush()).is_err() {
            gone.store(true, Ordering::SeqCst);
            return;
        }
    }
}

/// The child, whatever a panicking holder left behind.
fn lock(child: &Mutex<Child>) -> std::sync::MutexGuard<'_, Child> {
    child.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests;
