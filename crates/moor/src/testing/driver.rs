//! In-memory `Driver` for unit tests of ops.
#![allow(
    dead_code,
    clippy::significant_drop_tightening,
    reason = "test fake; not yet used by ops"
)]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use moor_core::box_key::CRED_SWEEP_SCRIPT;
use moor_core::seam::driver::{
    AcquireReq, Acquired, BoxListing, BoxStep, Driver, DriverCaps, DriverError, ExecOutcome,
    ExecSpec, Released, SleepKind, SleepOutcome, StepDone,
};
use moor_core::seam::proc::{Deadline, Output};
use moor_core::{BoxHandle, BoxPath, ProviderId};

pub(crate) use script::PutCall;
use script::{layout, write_bytes, PeekCall};
pub(crate) use tunnel::TunnelReply;

#[derive(Default)]
struct State {
    next: u32,
    live: Vec<String>,
    providers: Vec<(String, ProviderId)>,
    calls: Vec<String>,
    rec: script::Rec,
    fail_acquire: Option<DriverError>,
    fail_release: Option<DriverError>,
    fail_release_always: Option<DriverError>,
    fail_list: Option<DriverError>,
    fail_copy_out: Option<DriverError>,
    copy_content: Option<Vec<u8>>,
    copy_symlink: Option<PathBuf>,
    exec_results: VecDeque<Result<ExecOutcome, DriverError>>,
    exec_stdout: VecDeque<String>,
    execs: Vec<(BoxHandle, ExecSpec, Deadline)>,
    sweeps: Vec<(BoxHandle, ExecSpec, Deadline)>,
    fail_sweep: Option<DriverError>,
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
    after_exec: Option<Arc<ExecHook>>,
    on_wake: Option<Arc<ExecHook>>,
    work_root: Option<String>,
    fail_workdir: Option<DriverError>,
    tunnel: tunnel::TunnelState,
    sleep_kind: Option<SleepKind>,
}

/// One `copy_out` call: handle, source, destination and host cwd.
type CopyCall = (BoxHandle, BoxPath, PathBuf, PathBuf);

/// Runs inside every `exec`, before its result is served.
type ExecHook = dyn Fn() + Send + Sync;

/// Scripted Driver: boxes `box-1`, `box-2`, ... with workdir `/work/cbx_N/<repo>`
/// unless [`FakeDriver::work_root`] says otherwise; the lease id differs from
/// the handle so a path rebuilt from the handle is caught.
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

    /// Makes every `copy_out` write `content` to its destination.
    pub(crate) fn copy_text(self, content: impl AsRef<[u8]>) -> Self {
        self.st().copy_content = Some(content.as_ref().to_vec());
        self
    }

    /// Makes every `copy_out` leave a symlink to `target` at its destination.
    pub(crate) fn copy_symlink(self, target: impl Into<PathBuf>) -> Self {
        self.st().copy_symlink = Some(target.into());
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

    /// Queues text that later `exec` calls write to their `Output::Log` file, served in order.
    pub(crate) fn exec_stdout(self, out: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.st()
            .exec_stdout
            .extend(out.into_iter().map(Into::into));
        self
    }

    /// Makes `handle` vanish, as if deleted behind moor's back.
    pub(crate) fn forget(&self, handle: &BoxHandle) {
        self.st().live.retain(|h| h != handle.as_str());
    }

    /// Calls so far, like `acquire`, `release box-1` or `sweep box-1`.
    pub(crate) fn calls(&self) -> Vec<String> {
        self.st().calls.clone()
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
        Ok(self.caps_reply())
    }

    fn acquire(&self, req: &AcquireReq) -> Result<Acquired, DriverError> {
        let mut s = self.st();
        s.note("acquire".to_owned());
        if let Some(e) = s.fail_acquire.take() {
            return Err(e);
        }
        s.next += 1;
        let id = format!("box-{}", s.next);
        s.live.push(id.clone());
        s.providers.push((id.clone(), req.provider.clone()));
        let workdir = layout(s.work_root.as_deref(), &id, req.worktree.as_deref())?;
        Ok(Acquired {
            handle: BoxHandle::new(id),
            workdir,
        })
    }

    fn planned_handle(&self, _req: &AcquireReq) -> BoxHandle {
        BoxHandle::new(format!("box-{}", self.st().next + 1))
    }

    fn release(&self, handle: &BoxHandle, _: Option<&Path>) -> Result<Released, DriverError> {
        let mut s = self.st();
        s.note(format!("release {}", handle.as_str()));
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
        s.note("list".to_owned());
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

    fn workdir(&self, handle: &BoxHandle, worktree: &Path) -> Result<BoxPath, DriverError> {
        let mut s = self.st();
        s.note(format!("workdir {}", handle.as_str()));
        if let Some(e) = s.fail_workdir.take() {
            return Err(e);
        }
        if !s.live.iter().any(|h| h == handle.as_str()) {
            return Err(DriverError::NotFound);
        }
        layout(s.work_root.as_deref(), handle.as_str(), Some(worktree))
    }

    fn exec(
        &self,
        handle: &BoxHandle,
        spec: &ExecSpec,
        deadline: Deadline,
    ) -> Result<ExecOutcome, DriverError> {
        if spec.argv == ["sh", "-c", CRED_SWEEP_SCRIPT] {
            return self.sweep_exec(handle, spec, deadline);
        }
        let hook = self.st().on_exec.clone();
        if let Some(hook) = hook {
            hook();
        }
        let mut s = self.st();
        s.note(format!("exec {}", handle.as_str()));
        s.execs.push((handle.clone(), spec.clone(), deadline));
        if !s.live.iter().any(|h| h == handle.as_str()) {
            return Err(DriverError::NotFound);
        }
        if let (Some(Output::Log(path)), Some(text)) = (&spec.output, s.exec_stdout.pop_front()) {
            write_bytes(path, text.as_bytes())?;
        }
        let out = s
            .exec_results
            .pop_front()
            .unwrap_or(Ok(ExecOutcome::Exited { code: 0 }));
        let hook = s.after_exec.clone();
        drop(s);
        if let Some(hook) = hook {
            hook();
        }
        out
    }

    fn copy_out(
        &self,
        handle: &BoxHandle,
        from: &BoxPath,
        to: &Path,
        host_cwd: &Path,
    ) -> Result<(), DriverError> {
        let mut s = self.st();
        s.note(format!("copy_out {}", handle.as_str()));
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

    fn peek_file(
        &self,
        handle: &BoxHandle,
        path: &BoxPath,
        host_cwd: &Path,
    ) -> Result<Option<String>, DriverError> {
        let mut s = self.st();
        s.note(format!("peek_file {}", handle.as_str()));
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
        s.note(format!("put_file {}", handle.as_str()));
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
        s.note(format!("connect_argv {}", handle.as_str()));
        if !s.live.iter().any(|h| h == handle.as_str()) {
            return Err(DriverError::NotFound);
        }
        if let Some(e) = s.fail_connect.take() {
            return Err(e);
        }
        Ok(["ssh", "-t", handle.as_str()].map(str::to_owned).to_vec())
    }

    fn batch(
        &self,
        handle: &BoxHandle,
        connect: &[String],
        steps: &[BoxStep],
        host_cwd: &Path,
        deadline: Deadline,
    ) -> Vec<Result<StepDone, DriverError>> {
        self.run_batch(handle, connect, steps, host_cwd, deadline)
    }

    fn sleep(&self, handle: &BoxHandle, _: Option<&Path>) -> Result<SleepOutcome, DriverError> {
        let mut s = self.st();
        s.note(format!("sleep {}", handle.as_str()));
        if !s.live.iter().any(|h| h == handle.as_str()) {
            return Err(DriverError::NotFound);
        }
        if let Some(e) = s.fail_sleep.take() {
            return Err(e);
        }
        Ok(s.sleep_outcome.clone().unwrap_or(SleepOutcome::Slept))
    }

    fn wake(&self, handle: &BoxHandle, _: Option<&Path>) -> Result<(), DriverError> {
        let mut s = self.st();
        s.note(format!("wake {}", handle.as_str()));
        if !s.live.iter().any(|h| h == handle.as_str()) {
            return Err(DriverError::NotFound);
        }
        if let Some(e) = s.fail_wake.take() {
            return Err(e);
        }
        let hook = s.on_wake.clone();
        drop(s);
        if let Some(hook) = hook {
            hook();
        }
        Ok(())
    }
}

mod knobs;
mod script;
#[cfg(test)]
mod tests;
mod tunnel;
