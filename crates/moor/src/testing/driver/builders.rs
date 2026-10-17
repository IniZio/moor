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
}
