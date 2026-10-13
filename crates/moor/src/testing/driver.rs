//! In-memory `Driver` for unit tests of ops.
#![allow(
    dead_code,
    clippy::significant_drop_tightening,
    reason = "test fake; not yet used by ops"
)]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use moor_core::seam::driver::{
    AcquireReq, Acquired, BoxListing, Driver, DriverCaps, DriverError, EgressApplied, ExecOutcome,
    ExecSpec, Released, SleepOutcome,
};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxHandle, BoxPath, ProviderId};

use script::PeekCall;
pub(crate) use script::PutCall;

#[derive(Default)]
struct State {
    next: u32,
    live: Vec<String>,
    providers: Vec<(String, ProviderId)>,
    calls: Vec<String>,
    fail_acquire: Option<DriverError>,
    fail_release: Option<DriverError>,
    fail_release_always: Option<DriverError>,
    fail_list: Option<DriverError>,
    fail_copy_out: Option<DriverError>,
    copy_content: Option<String>,
    exec_results: VecDeque<Result<ExecOutcome, DriverError>>,
    execs: Vec<(BoxHandle, ExecSpec, Deadline)>,
    copies: Vec<CopyCall>,
    files: Vec<(String, String, String)>,
    peeks: Vec<PeekCall>,
    puts: Vec<PutCall>,
    fail_put: Option<DriverError>,
    fail_connect: Option<DriverError>,
    fail_peek: Option<DriverError>,
    fail_sleep: Option<DriverError>,
    fail_wake: Option<DriverError>,
    sleep_outcome: Option<SleepOutcome>,
    on_exec: Option<Arc<ExecHook>>,
    work_root: Option<String>,
    fail_workdir: Option<DriverError>,
    tunnel: tunnel::TunnelState,
}

/// One `copy_out` call: handle, source, destination and host cwd.
type CopyCall = (BoxHandle, BoxPath, PathBuf, PathBuf);

/// Runs inside every `exec`, before its result is served.
type ExecHook = dyn Fn() + Send + Sync;

/// Scripted Driver: boxes `box-1`, `box-2`, ... with workdir `/work/box-N`
/// unless [`FakeDriver::work_root`] says otherwise.
#[derive(Default)]
pub(crate) struct FakeDriver {
    state: Mutex<State>,
}

impl FakeDriver {
    /// An empty Driver with no scripted failures.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn st(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
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

    /// Makes every `copy_out` write `text` to its destination.
    pub(crate) fn copy_text(self, text: &str) -> Self {
        self.st().copy_content = Some(text.to_owned());
        self
    }

    /// Queues scripted `exec` results, served in order; then exit code 0.
    pub(crate) fn exec_results(
        self,
        rs: impl IntoIterator<Item = Result<ExecOutcome, DriverError>>,
    ) -> Self {
        self.st().exec_results.extend(rs);
        self
    }

    /// Makes `handle` vanish, as if deleted behind moor's back.
    pub(crate) fn forget(&self, handle: &BoxHandle) {
        self.st().live.retain(|h| h != handle.as_str());
    }

    /// Calls so far, like `acquire` or `release box-1`.
    pub(crate) fn calls(&self) -> Vec<String> {
        self.st().calls.clone()
    }

    /// Runs `f` at the start of every later `exec`, outside the Driver's lock.
    pub(crate) fn on_exec(&self, f: impl Fn() + Send + Sync + 'static) {
        self.st().on_exec = Some(Arc::new(f));
    }

    /// Every `exec` so far: handle, spec and deadline, in call order.
    pub(crate) fn execs(&self) -> Vec<(BoxHandle, ExecSpec, Deadline)> {
        self.st().execs.clone()
    }

    /// Every `copy_out` so far: handle, source, destination, host cwd.
    pub(crate) fn copies(&self) -> Vec<CopyCall> {
        self.st().copies.clone()
    }
}

impl Driver for FakeDriver {
    fn caps(&self, _provider: &ProviderId) -> Result<DriverCaps, DriverError> {
        self.st().calls.push("caps".to_owned());
        Ok(self.caps_reply())
    }

    fn acquire(&self, req: &AcquireReq) -> Result<Acquired, DriverError> {
        let mut s = self.st();
        s.calls.push("acquire".to_owned());
        if let Some(e) = s.fail_acquire.take() {
            return Err(e);
        }
        s.next += 1;
        let id = format!("box-{}", s.next);
        s.live.push(id.clone());
        s.providers.push((id.clone(), req.provider.clone()));
        let root = s.work_root.as_deref().unwrap_or("/work");
        let workdir = BoxPath::under(root, &id).map_err(|e| DriverError::Failed {
            summary: e.to_string(),
        })?;
        Ok(Acquired {
            handle: BoxHandle::new(id),
            workdir,
            egress: EgressApplied::NotEnforced,
        })
    }

    fn planned_handle(&self, _req: &AcquireReq) -> BoxHandle {
        BoxHandle::new(format!("box-{}", self.st().next + 1))
    }

    fn release(&self, handle: &BoxHandle) -> Result<Released, DriverError> {
        let mut s = self.st();
        s.calls.push(format!("release {}", handle.as_str()));
        if let Some(e) = s
            .fail_release_always
            .clone()
            .or_else(|| s.fail_release.take())
        {
            return Err(e);
        }
        let before = s.live.len();
        s.live.retain(|h| h != handle.as_str());
        Ok(if s.live.len() < before {
            Released::Deleted
        } else {
            Released::AlreadyGone
        })
    }

    fn list(&self, provider: &ProviderId) -> Result<Vec<BoxListing>, DriverError> {
        let mut s = self.st();
        s.calls.push("list".to_owned());
        if let Some(e) = s.fail_list.take() {
            return Err(e);
        }
        Ok(s.providers
            .iter()
            .filter(|(h, p)| p == provider && s.live.contains(h))
            .map(|(h, _)| BoxListing {
                handle: BoxHandle::new(h.clone()),
                provider: Some(provider.clone()),
                asleep: false,
            })
            .collect())
    }

    fn reverse_tunnel(&self, h: &BoxHandle, bp: u16, hp: u16, cwd: &Path) -> tunnel::Opened {
        self.tunnel_request(h, bp, hp, cwd)
    }

    fn workdir(&self, handle: &BoxHandle) -> Result<BoxPath, DriverError> {
        let mut s = self.st();
        s.calls.push(format!("workdir {}", handle.as_str()));
        if let Some(e) = s.fail_workdir.take() {
            return Err(e);
        }
        if !s.live.iter().any(|h| h == handle.as_str()) {
            return Err(DriverError::NotFound);
        }
        BoxPath::under(s.work_root.as_deref().unwrap_or("/work"), handle.as_str()).map_err(|e| {
            DriverError::Failed {
                summary: e.to_string(),
            }
        })
    }

    fn exec(
        &self,
        handle: &BoxHandle,
        spec: &ExecSpec,
        deadline: Deadline,
    ) -> Result<ExecOutcome, DriverError> {
        let hook = self.st().on_exec.clone();
        if let Some(hook) = hook {
            hook();
        }
        let mut s = self.st();
        s.calls.push(format!("exec {}", handle.as_str()));
        s.execs.push((handle.clone(), spec.clone(), deadline));
        if !s.live.iter().any(|h| h == handle.as_str()) {
            return Err(DriverError::NotFound);
        }
        s.exec_results
            .pop_front()
            .unwrap_or(Ok(ExecOutcome::Exited { code: 0 }))
    }

    fn copy_out(
        &self,
        handle: &BoxHandle,
        from: &BoxPath,
        to: &Path,
        host_cwd: &Path,
    ) -> Result<(), DriverError> {
        let mut s = self.st();
        s.calls.push(format!("copy_out {}", handle.as_str()));
        s.copies.push((
            handle.clone(),
            from.clone(),
            to.to_path_buf(),
            host_cwd.to_path_buf(),
        ));
        if !s.live.iter().any(|h| h == handle.as_str()) {
            return Err(DriverError::NotFound);
        }
        if let Some(e) = s.fail_copy_out.take() {
            return Err(e);
        }
        s.copy_content
            .as_deref()
            .map_or(Ok(()), |text| write_text(to, text))
    }

    fn peek_file(
        &self,
        handle: &BoxHandle,
        path: &BoxPath,
        host_cwd: &Path,
    ) -> Result<Option<String>, DriverError> {
        let mut s = self.st();
        s.calls.push(format!("peek_file {}", handle.as_str()));
        s.peeks
            .push((handle.clone(), path.clone(), host_cwd.to_path_buf()));
        if !s.live.iter().any(|h| h == handle.as_str()) {
            return Err(DriverError::NotFound);
        }
        if let Some(e) = s.fail_peek.take() {
            return Err(e);
        }
        Ok(s.files
            .iter()
            .find(|(h, p, _)| h == handle.as_str() && p == path.as_str())
            .map(|(_, _, text)| text.clone()))
    }

    fn put_file(
        &self,
        handle: &BoxHandle,
        path: &BoxPath,
        bytes: &[u8],
        mode: u32,
        host_cwd: &Path,
    ) -> Result<(), DriverError> {
        let mut s = self.st();
        s.calls.push(format!("put_file {}", handle.as_str()));
        s.puts.push((
            handle.clone(),
            path.clone(),
            bytes.to_vec(),
            mode,
            host_cwd.to_path_buf(),
        ));
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

    fn connect_argv(
        &self,
        handle: &BoxHandle,
        _host_cwd: &Path,
    ) -> Result<Vec<String>, DriverError> {
        let mut s = self.st();
        s.calls.push(format!("connect_argv {}", handle.as_str()));
        if !s.live.iter().any(|h| h == handle.as_str()) {
            return Err(DriverError::NotFound);
        }
        if let Some(e) = s.fail_connect.take() {
            return Err(e);
        }
        Ok(["ssh", "-t", handle.as_str()].map(str::to_owned).to_vec())
    }

    fn sleep(&self, handle: &BoxHandle) -> Result<SleepOutcome, DriverError> {
        let mut s = self.st();
        s.calls.push(format!("sleep {}", handle.as_str()));
        if !s.live.iter().any(|h| h == handle.as_str()) {
            return Err(DriverError::NotFound);
        }
        if let Some(e) = s.fail_sleep.take() {
            return Err(e);
        }
        Ok(s.sleep_outcome.clone().unwrap_or(SleepOutcome::Slept))
    }

    fn wake(&self, handle: &BoxHandle) -> Result<(), DriverError> {
        let mut s = self.st();
        s.calls.push(format!("wake {}", handle.as_str()));
        if !s.live.iter().any(|h| h == handle.as_str()) {
            return Err(DriverError::NotFound);
        }
        s.fail_wake.take().map_or(Ok(()), Err)
    }
}

/// Writes `text` to `to`; like a real copy, fails when the destination directory is missing.
fn write_text(to: &Path, text: &str) -> Result<(), DriverError> {
    std::fs::write(to, text).map_err(|e| DriverError::Failed {
        summary: format!("copy destination parent is unavailable: {e}"),
    })
}

mod script;
#[cfg(test)]
mod tests;
mod tunnel;
