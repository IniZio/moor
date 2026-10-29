//! Setup knobs for the Driver fake: where boxes live and which calls fail, and the calls it saw.

use std::sync::Arc;

use moor_core::cache::CachePlan;
use moor_core::seam::driver::{DriverError, ExecSpec};
use moor_core::seam::proc::{Deadline, Output};
use moor_core::BoxHandle;

use super::{CopyCall, FakeDriver, State};

impl State {
    /// The text an `exec` writes to its log. An artifact index exec takes the next artifact
    /// index text, or writes an empty file; any other exec takes the next scripted stdout.
    pub(super) fn log_text(&mut self, spec: &ExecSpec) -> Option<String> {
        if !matches!(spec.output, Some(Output::Log(_))) {
            return None;
        }
        if is_artifact_index(spec) {
            return Some(self.artifact_index.pop_front().unwrap_or_default());
        }
        self.exec_stdout.pop_front()
    }
}

/// Whether the `sh -c` script of `spec` reads the Box's `.moor/artifacts` directory.
fn is_artifact_index(spec: &ExecSpec) -> bool {
    matches!(spec.argv.as_slice(), [sh, c, script, ..]
        if sh == "sh" && c == "-c" && script.contains(".moor/artifacts"))
}

/// The fake's cache volume capability and the plans and nonces `acquire` received.
#[derive(Default)]
pub(super) struct CacheState {
    /// Whether `caps` reports cache volumes.
    pub(super) on: bool,
    /// The plan of every `acquire`, in call order.
    pub(super) plans: Vec<CachePlan>,
    /// The nonce of every `acquire`, in call order.
    pub(super) nonces: Vec<String>,
}

impl FakeDriver {
    /// Makes every `copy_out` write `content` to its destination.
    pub(crate) fn copy_text(self, content: impl AsRef<[u8]>) -> Self {
        self.st().copy_content = Some(content.as_ref().to_vec());
        self
    }

    /// Makes every `copy_out` leave a symlink to `target` at its destination.
    pub(crate) fn copy_symlink(self, target: impl Into<std::path::PathBuf>) -> Self {
        self.st().copy_symlink = Some(target.into());
        self
    }

    /// Reports that the provider keeps cache volumes.
    pub(crate) fn cache_volumes(self) -> Self {
        self.st().cache.on = true;
        self
    }

    /// The cache plan of every `acquire` so far, in call order.
    pub(crate) fn acquired_plans(&self) -> Vec<CachePlan> {
        self.st().cache.plans.clone()
    }

    /// The nonce of every `acquire` so far, in call order.
    pub(crate) fn acquired_nonces(&self) -> Vec<String> {
        self.st().cache.nonces.clone()
    }

    /// Every `exec` so far: handle, spec and deadline, in call order.
    pub(crate) fn execs(&self) -> Vec<(BoxHandle, ExecSpec, Deadline)> {
        self.st().execs.clone()
    }

    /// Every `copy_out` so far: handle, source and destination.
    pub(crate) fn copies(&self) -> Vec<CopyCall> {
        self.st().copies.clone()
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

    /// Every `acquire` reports `notes`, then runs `during` before it returns.
    pub(crate) fn notes_while_acquiring(
        &self,
        notes: &[&str],
        during: impl Fn() + Send + Sync + 'static,
    ) {
        let mut s = self.st();
        s.acquire_notes = notes.iter().map(|n| (*n).to_owned()).collect();
        s.during_acquire = Some(Arc::new(during));
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

    /// Every later `list` fails with `e`.
    pub(crate) fn fail_list_always(&self, e: DriverError) {
        self.st().fail_list_always = Some(e);
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

    /// Queues text that later non-artifact `exec` calls write to their `Output::Log` file,
    /// served in order.
    pub(crate) fn exec_stdout(self, out: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.st()
            .exec_stdout
            .extend(out.into_iter().map(Into::into));
        self
    }

    /// Queues the listings the artifact index exec writes, served in order. Only an exec whose
    /// `sh -c` script reads `.moor/artifacts` consumes them; it writes an empty file once none remain.
    pub(crate) fn artifact_index(self, out: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.st()
            .artifact_index
            .extend(out.into_iter().map(Into::into));
        self
    }

    /// Queues one more artifact index listing, without consuming the driver.
    pub(crate) fn push_artifact_index(&self, text: impl Into<String>) {
        self.st().artifact_index.push_back(text.into());
    }
}
