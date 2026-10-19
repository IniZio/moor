//! Setup knobs for the Driver fake: where boxes live and which calls fail.

use moor_core::seam::driver::{DriverError, ExecSpec};
use moor_core::seam::proc::Output;

use super::{FakeDriver, State};

impl State {
    /// The text an `exec` writes to its log: the next scripted stdout, or nothing for an
    /// artifact index with none scripted.
    pub(super) fn log_text(&mut self, spec: &ExecSpec) -> Option<String> {
        let empty = spec.argv.iter().any(|a| a.contains(".moor/artifacts"));
        matches!(spec.output, Some(Output::Log(_)))
            .then(|| {
                self.exec_stdout
                    .pop_front()
                    .or_else(|| empty.then(String::new))
            })
            .flatten()
    }
}

impl FakeDriver {
    /// Puts every box workdir under `root` instead of `/work`.
    pub(crate) fn work_root(self, root: &str) -> Self {
        self.st().work_root = Some(root.to_owned());
        self
    }

    /// Moves where later `workdir` lookups report every box.
    pub(crate) fn set_work_root(&self, root: &str) {
        self.st().work_root = Some(root.to_owned());
    }

    /// The next `workdir` fails once with `e`.
    pub(crate) fn fail_workdir(&self, e: DriverError) {
        self.st().fail_workdir = Some(e);
    }

    /// The next `acquire` fails once with `e`.
    pub(crate) fn fail_acquire(self, e: DriverError) -> Self {
        self.st().fail_acquire = Some(e);
        self
    }

    /// The next `release` fails once with `e`.
    pub(crate) fn fail_release(self, e: DriverError) -> Self {
        self.st().fail_release = Some(e);
        self
    }

    /// Every `release` fails with `e`.
    pub(crate) fn fail_release_always(self, e: DriverError) -> Self {
        self.st().fail_release_always = Some(e);
        self
    }

    /// The next `list` fails once with `e`.
    pub(crate) fn fail_list(self, e: DriverError) -> Self {
        self.st().fail_list = Some(e);
        self
    }

    /// The next `copy_out` fails once with `e`.
    pub(crate) fn fail_copy_out(self, e: DriverError) -> Self {
        self.st().fail_copy_out = Some(e);
        self
    }

    /// Replaces the text later `copy_out` calls write, without consuming the driver.
    pub(crate) fn set_copy_text(&self, content: impl AsRef<[u8]>) {
        let mut s = self.st();
        s.copy_content = Some(content.as_ref().to_vec());
        s.copy_symlink = None;
    }

    /// Queues more text for later `exec` calls to write to their `Output::Log` file.
    pub(crate) fn push_exec_stdout(&self, out: impl Into<String>) {
        self.st().exec_stdout.push_back(out.into());
    }
}
