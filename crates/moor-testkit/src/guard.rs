//! A guard that keeps a test's spawned child from outliving the test.

use std::io::{self, Read};
use std::ops::{Deref, DerefMut};
use std::process::{Child, ExitStatus, Output};

use rustix::process::{kill_process, Pid, Signal};

/// Owns a spawned [`Child`] and kills and reaps it on drop, so a panicking test leaves
/// nothing behind. A bare `Child` is not killed when dropped.
///
/// It derefs to the [`Child`], so a test reads its pipes and waits on it as before.
#[derive(Debug)]
pub struct ChildGuard(Child);

impl ChildGuard {
    /// Takes ownership of `child`.
    #[must_use]
    pub const fn new(child: Child) -> Self {
        Self(child)
    }

    /// Sends `SIGTERM` to the child and waits for it to exit.
    pub fn terminate(&mut self) -> io::Result<ExitStatus> {
        if let Some(pid) = self.pid() {
            let _ = kill_process(pid, Signal::TERM);
        }
        self.0.wait()
    }

    /// Closes the child's stdin, reads its stdout and stderr to the end and waits for it, like
    /// [`Child::wait_with_output`] but through the guard, which still kills the child on a panic.
    pub fn wait_with_output(&mut self) -> io::Result<Output> {
        drop(self.0.stdin.take());
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let (out_pipe, err_pipe) = (self.0.stdout.take(), self.0.stderr.take());
        let err = std::thread::scope(|scope| {
            let reader = scope.spawn(|| err_pipe.map_or(Ok(0), |mut p| p.read_to_end(&mut stderr)));
            let out = out_pipe.map_or(Ok(0), |mut p| p.read_to_end(&mut stdout));
            let err = reader
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("stderr reader panicked")));
            out.and(err)
        });
        let status = self.0.wait()?;
        err?;
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }

    /// The child's pid, which is always positive.
    fn pid(&self) -> Option<Pid> {
        Pid::from_raw(i32::try_from(self.0.id()).ok()?)
    }
}

impl Deref for ChildGuard {
    type Target = Child;

    fn deref(&self) -> &Child {
        &self.0
    }
}

impl DerefMut for ChildGuard {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            if let Some(pid) = self.pid() {
                let _ = kill_process(pid, Signal::KILL);
            }
            let _ = self.0.wait();
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test code: a failed setup step should abort the test"
)]
mod tests {
    use std::io::Write;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::process::Command;

    use rustix::process::test_kill_process;

    use super::*;

    fn gone(pid: Pid) -> bool {
        test_kill_process(pid).is_err()
    }

    #[test]
    fn a_panic_in_the_guarded_scope_kills_the_child() {
        let mut seen = None;
        let result = catch_unwind(AssertUnwindSafe(|| {
            let guard = ChildGuard::new(Command::new("sleep").arg("600").spawn().unwrap());
            seen = guard.pid();
            assert!(seen.is_some_and(|p| !gone(p)));
            panic!("the test fails with the child running");
        }));
        assert!(result.is_err());
        assert!(gone(seen.unwrap()), "child outlived the panicking scope");
    }

    #[test]
    fn terminate_reports_the_signal_exit() {
        let mut guard = ChildGuard::new(Command::new("sleep").arg("600").spawn().unwrap());
        let status = guard.terminate().unwrap();
        assert!(!status.success());
    }

    #[test]
    fn wait_with_output_collects_both_pipes_through_the_guard() {
        let child = Command::new("sh")
            .args(["-c", "cat; echo err >&2"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut guard = ChildGuard::new(child);
        guard.stdin.as_mut().unwrap().write_all(b"in\n").unwrap();
        let out = guard.wait_with_output().unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout, b"in\n");
        assert_eq!(out.stderr, b"err\n");
    }

    #[test]
    fn dropping_an_exited_child_is_quiet() {
        let mut guard = ChildGuard::new(Command::new("true").spawn().unwrap());
        assert!(guard.wait().unwrap().success());
    }
}
