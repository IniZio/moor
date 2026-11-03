//! A guard that keeps a test's spawned child from outliving the test.

use std::io;
use std::ops::{Deref, DerefMut};
use std::process::{Child, ExitStatus};

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
    fn dropping_an_exited_child_is_quiet() {
        let mut guard = ChildGuard::new(Command::new("true").spawn().unwrap());
        assert!(guard.wait().unwrap().success());
    }
}
