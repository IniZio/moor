//! Platform and host-kind knobs for the Driver fake.

use moor_core::seam::driver::Platform;

use super::FakeDriver;

impl FakeDriver {
    /// Makes every later `acquire` report `platform` instead of Linux.
    pub(crate) fn set_platform(&self, platform: Platform) {
        self.st().platform = platform;
    }

    /// Reports the provider as a static host.
    pub(crate) fn static_host(self) -> Self {
        self.st().static_host = true;
        self
    }

    /// Makes `caps` fail with `Unavailable`.
    pub(crate) fn fail_caps(self) -> Self {
        self.st().fail_caps = true;
        self
    }

    /// Queues text that later `exec` calls write to their `Output::Log` file, served in order.
    pub(crate) fn exec_stdout(self, out: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.st()
            .exec_stdout
            .extend(out.into_iter().map(Into::into));
        self
    }
}
