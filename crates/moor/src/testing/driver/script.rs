//! Scripted failures and recorded calls for the Driver fake.

use std::path::PathBuf;

use moor_core::seam::driver::{DriverError, SleepOutcome};
use moor_core::{BoxHandle, BoxPath};

use super::FakeDriver;

/// One `peek_file` call: handle, path and host cwd.
pub(super) type PeekCall = (BoxHandle, BoxPath, PathBuf);

/// One `put_file` call: handle, path, content, mode and host cwd.
pub(crate) type PutCall = (BoxHandle, BoxPath, Vec<u8>, u32, PathBuf);

impl FakeDriver {
    /// Makes `peek_file` of `path` in the Box `handle` return `text`.
    pub(crate) fn peek_text(self, handle: &str, path: &BoxPath, text: &str) -> Self {
        self.st()
            .files
            .push((handle.to_owned(), path.as_str().to_owned(), text.to_owned()));
        self
    }

    /// The next `peek_file` fails once with `e`.
    pub(crate) fn fail_peek(self, e: DriverError) -> Self {
        self.st().fail_peek = Some(e);
        self
    }

    /// The next `put_file` fails once with `e`.
    pub(crate) fn fail_put(self, e: DriverError) -> Self {
        self.st().fail_put = Some(e);
        self
    }

    /// The next `connect_argv` fails once with `e`.
    pub(crate) fn fail_connect(self, e: DriverError) -> Self {
        self.st().fail_connect = Some(e);
        self
    }

    /// Every `peek_file` so far: handle, path and host cwd, in call order.
    pub(crate) fn peeks(&self) -> Vec<PeekCall> {
        self.st().peeks.clone()
    }

    /// Every `put_file` so far, in call order.
    pub(crate) fn puts(&self) -> Vec<PutCall> {
        self.st().puts.clone()
    }

    /// The next `sleep` fails once with `e`.
    pub(crate) fn fail_sleep(self, e: DriverError) -> Self {
        self.st().fail_sleep = Some(e);
        self
    }

    /// The next `wake` fails once with `e`.
    pub(crate) fn fail_wake(self, e: DriverError) -> Self {
        self.st().fail_wake = Some(e);
        self
    }

    /// Makes every `sleep` report `outcome` instead of `Slept`.
    pub(crate) fn sleep_outcome(self, outcome: SleepOutcome) -> Self {
        self.st().sleep_outcome = Some(outcome);
        self
    }
}
