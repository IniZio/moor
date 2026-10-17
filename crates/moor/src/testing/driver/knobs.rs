//! Setup knobs for the Driver fake: where boxes live and which calls fail.

use moor_core::seam::driver::DriverError;
use moor_core::BoxHandle;

use super::FakeDriver;

impl FakeDriver {
    /// Brings a released Box back, as if the release had left it running.
    pub(crate) fn revive(&self, handle: &BoxHandle) {
        self.st().live.push(handle.as_str().to_owned());
    }

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
}
