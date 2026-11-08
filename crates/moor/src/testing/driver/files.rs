//! Files the Driver fake keeps for each Box, and the leases it was asked to touch.

use moor_core::seam::driver::DriverError;
use moor_core::{BoxHandle, BoxPath};

use super::FakeDriver;

impl FakeDriver {
    /// The Boxes whose lease was touched, in order.
    pub(crate) fn touches(&self) -> Vec<BoxHandle> {
        self.st().touches.clone()
    }

    /// Writes `bytes` at `path` on `handle`, as `put_file` does.
    pub(super) fn put(
        &self,
        handle: &BoxHandle,
        path: &BoxPath,
        bytes: &[u8],
        mode: u32,
    ) -> Result<(), DriverError> {
        let mut s = self.st();
        s.note(format!("put_file {}", handle.as_str()));
        s.puts
            .push((handle.clone(), path.clone(), bytes.to_vec(), mode));
        if !s.live.iter().any(|h| h == handle.as_str()) {
            return Err(DriverError::NotFound);
        }
        if let Some(e) = s.fail_put.take() {
            return Err(e);
        }
        let (h, p) = (handle.as_str(), path.as_str());
        let text = String::from_utf8_lossy(bytes).into_owned();
        match s.files.iter_mut().find(|(fh, fp, _)| fh == h && fp == p) {
            Some(f) => f.2 = text,
            None => s.files.push((h.to_owned(), p.to_owned(), text)),
        }
        Ok(())
    }
}
