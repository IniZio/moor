//! Files the Driver fake keeps for each Box, and the leases it was asked to touch.

use std::path::Path;

use moor_core::seam::driver::{DriverError, Place};
use moor_core::{BoxHandle, BoxPath};

use super::script::write_bytes;

use super::FakeDriver;

impl FakeDriver {
    /// Copies `from` out of `handle` to `to`, as `copy_out` does.
    pub(super) fn copy(
        &self,
        handle: &BoxHandle,
        place: Place,
        from: &BoxPath,
        to: &Path,
    ) -> Result<(), DriverError> {
        self.run_copy_out_hook();
        let mut s = self.st();
        s.note(format!("copy_out {}", handle.as_str()));
        s.placed.push(("copy_out", place));
        s.copies
            .push((handle.clone(), from.clone(), to.to_path_buf()));
        if !s.live.iter().any(|h| h == handle.as_str()) {
            return Err(DriverError::NotFound);
        }
        if let Some(e) = s.fail_copy_out.take() {
            return Err(e);
        }
        if let Some(trees) = s.trees.clone() {
            let rel = from.as_str().rsplit("/repo/").next().unwrap_or_default();
            return trees.copy_out(handle.as_str(), rel, to);
        }
        if let Some(target) = &s.copy_symlink {
            let _ = std::fs::remove_file(to);
            return std::os::unix::fs::symlink(target, to).map_err(|e| DriverError::Failed {
                summary: format!("cannot place a symlink: {e}"),
            });
        }
        s.copy_content
            .as_deref()
            .map_or(Ok(()), |bytes| write_bytes(to, bytes))
    }

    /// The Boxes whose lease was touched, in order.
    pub(crate) fn touches(&self) -> Vec<BoxHandle> {
        self.st().touches.clone()
    }

    /// Writes `bytes` at `path` on `handle`, as `put_file` does.
    pub(super) fn put(
        &self,
        handle: &BoxHandle,
        place: Place,
        path: &BoxPath,
        bytes: &[u8],
        mode: u32,
    ) -> Result<(), DriverError> {
        let mut s = self.st();
        s.note(format!("put_file {}", handle.as_str()));
        s.placed.push(("put_file", place));
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
