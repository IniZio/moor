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
    AcquireReq, Acquired, BoxListing, BoxStep, Cancelled, Driver, DriverCaps, DriverError,
    ExecOutcome, ExecSpec, Followed, HandlesListing, Place, Released, RunEnd,
    SleepOutcome, StepDone,
};
use moor_core::seam::proc::{Deadline, Pipes};
use moor_core::{BoxHandle, BoxPath, ProviderId};

pub(crate) use runs::{eventually, live_in_group};
pub(crate) use script::{is_clone_exec, PutCall};
use script::{layout, write_log, CopyCall, ExecHook, PeekCall};

#[derive(Default)]
struct State {
    next: u32,
    live: Vec<String>,
    stopped: Vec<String>,
    away: Vec<String>,
    by_handle: bool,
    providers: Vec<(String, ProviderId)>,
    calls: Vec<String>,
    rec: script::Rec,
    fail_acquire: Option<DriverError>,
    acquire_notes: Vec<String>,
    during_acquire: Option<Arc<dyn Fn() + Send + Sync>>,
    fail_release: Option<DriverError>,
    fail_release_always: Option<DriverError>,
    fail_list: Option<DriverError>,
    fail_list_always: Option<DriverError>,
    fail_copy_out: Option<DriverError>,
    copy_content: Option<Vec<u8>>,
    copy_symlink: Option<PathBuf>,
    /// Acquires that go on without their caller.
    flight: acquire::Flight,
    exec_results: VecDeque<Result<ExecOutcome, DriverError>>,
    /// Outcomes that end any `exec` whose argv mentions the needle.
    exec_ends: Vec<(String, ExecOutcome)>,
    exec_stdout: VecDeque<String>,
    artifact_index: VecDeque<String>,
    execs: Vec<(BoxHandle, ExecSpec, Deadline)>,
    /// The place each Box command was given, by the op that took it.
    placed: Vec<(&'static str, Place)>,
    refreshed_logins: Vec<(BoxHandle, usize)>,
    touches: Vec<BoxHandle>,
    copies: Vec<CopyCall>,
    files: Vec<(String, String, String)>,
    peeks: Vec<PeekCall>,
    puts: Vec<PutCall>,
    fail_put: Option<DriverError>,
    fail_connect: Option<DriverError>,
    fail_peek: Option<DriverError>,
    silent_peeks: VecDeque<DriverError>,
    fail_peek_of: Option<(BoxPath, DriverError)>,
    fail_sleep: Option<DriverError>,
    fail_wake: Option<DriverError>,
    sleep_outcome: Option<SleepOutcome>,
    on_exec: Option<Arc<ExecHook>>,
    after_exec: Option<Arc<ExecHook>>,
    on_wake: Option<Arc<ExecHook>>,
    on_sleep: Option<Arc<ExecHook>>,
    on_release: Option<Arc<ExecHook>>,
    work_root: Option<String>,
    fail_workdir: Option<DriverError>,
    cache: knobs::CacheState,
    runs: runs::RunState,
    exec_knobs: hooks::ExecKnobs,
    trees: Option<trees::Trees>,
    ports: ports::PortState,
}

/// Scripted Driver: boxes `box-1`, `box-2`, ... with workdir `/work/cbx_N/repo`
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
}

impl Driver for FakeDriver {
    fn caps(&self, _provider: &ProviderId) -> Result<DriverCaps, DriverError> {
        Ok(DriverCaps {
            has_workdir: true,
            cache_volumes: self.st().cache.on,
        })
    }

    fn acquire(&self, req: &AcquireReq) -> Result<Acquired, DriverError> {
        self.acquired(req)
    }

    fn acquire_noting(
        &self,
        req: &AcquireReq,
        note: &mut dyn FnMut(&str),
    ) -> Result<Acquired, DriverError> {
        self.take_off()?;
        self.land(req, note)
    }

    fn cancel_acquire(&self, handle: &BoxHandle) -> Result<(), DriverError> {
        self.st()
            .note(format!("cancel_acquire {}", handle.as_str()));
        Ok(())
    }

    fn acquire_outlives_caller(&self, _handle: &BoxHandle) -> bool {
        self.st().flight.goes_on
    }
    fn adopt_noting(
        &self,
        req: &AcquireReq,
        note: &mut dyn FnMut(&str),
    ) -> Result<Acquired, DriverError> {
        self.adopted(req, note)
    }

    fn planned_handle(&self, _req: &AcquireReq) -> BoxHandle {
        BoxHandle::new(format!("box-{}", self.st().next + 1))
    }

    fn manual_release(&self, handle: &BoxHandle) -> Option<String> {
        Some(format!("stop it from /leases/{}", handle.as_str()))
    }

    fn release(&self, handle: &BoxHandle) -> Result<Released, DriverError> {
        let hook = self.st().on_release.clone();
        if let Some(hook) = hook {
            hook();
        }
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
        self.listed(provider)
    }

    fn list_handles(&self, handles: &[BoxHandle]) -> Result<HandlesListing, DriverError> {
        self.listed_handles(handles)
    }


    fn workdir(&self, handle: &BoxHandle) -> Result<BoxPath, DriverError> {
        let mut s = self.st();
        s.note(format!("workdir {}", handle.as_str()));
        if let Some(e) = s.fail_workdir.take() {
            return Err(e);
        }
        if !s.live.iter().any(|h| h == handle.as_str()) {
            return Err(DriverError::NotFound);
        }
        layout(s.work_root.as_deref(), handle.as_str())
    }

    fn exec(
        &self,
        handle: &BoxHandle,
        spec: &ExecSpec,
        deadline: Deadline,
    ) -> Result<ExecOutcome, DriverError> {
        self.run_exec_hooks(spec);
        let mut s = self.st();
        s.note(format!("exec {}", handle.as_str()));
        s.placed.push(("exec", spec.place));
        s.execs.push((handle.clone(), spec.clone(), deadline));
        if !s.live.iter().any(|h| h == handle.as_str()) {
            return Err(DriverError::NotFound);
        }
        if s.trees.is_some() {
            return trees::exec_in_tree(s, handle.as_str(), spec);
        }
        let scripted = s.log_text(spec);
        let argv = spec.argv.join(" ");
        let ends = s.exec_ends.iter().find(|(n, _)| argv.contains(n.as_str()));
        if let Some(out) = ends.map(|(_, out)| *out).or_else(|| s.end_once(&argv)) {
            write_log(spec, scripted)?;
            return Ok(out);
        }
        let out = s
            .exec_results
            .pop_front()
            .unwrap_or(Ok(ExecOutcome::Exited { code: 0 }));
        write_log(spec, scripted)?;
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
        place: Place,
        from: &BoxPath,
        to: &Path,
    ) -> Result<(), DriverError> {
        self.copy(handle, place, from, to)
    }

    fn peek_file(
        &self,
        handle: &BoxHandle,
        place: Place,
        path: &BoxPath,
    ) -> Result<Option<String>, DriverError> {
        let mut s = self.st();
        s.note(format!("peek_file {}", handle.as_str()));
        s.placed.push(("peek_file", place));
        s.peeks.push((handle.clone(), path.clone()));
        if !s.live.iter().any(|h| h == handle.as_str()) {
            return Err(DriverError::NotFound);
        }
        if let Some(e) = s.peek_failure(path) {
            return Err(e);
        }
        Ok(s.files
            .iter()
            .find(|(h, p, _)| h == handle.as_str() && p == path.as_str())
            .map(|(_, _, text)| text.clone()))
    }

    fn put_file(
        &self,
        h: &BoxHandle,
        place: Place,
        p: &BoxPath,
        b: &[u8],
        mode: u32,
    ) -> Result<(), DriverError> {
        self.put(h, place, p, b, mode)
    }

    fn exec_piped(&self, h: &BoxHandle, s: &ExecSpec, p: Pipes) -> piped::Ended {
        self.piped(h, s, p)
    }

    fn touch(&self, handle: &BoxHandle) -> Result<(), DriverError> {
        self.st().touches.push(handle.clone());
        Ok(())
    }

    fn refresh_login(&self, handle: &BoxHandle) -> Result<(), DriverError> {
        let mut s = self.st();
        let execs = s.execs.len();
        s.refreshed_logins.push((handle.clone(), execs));
        Ok(())
    }

    fn connect_argv(&self, handle: &BoxHandle, place: Place) -> Result<Vec<String>, DriverError> {
        let mut s = self.st();
        s.note(format!("connect_argv {}", handle.as_str()));
        s.placed.push(("connect_argv", place));
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
        place: Place,
        steps: &[BoxStep],
        deadline: Deadline,
    ) -> Vec<Result<StepDone, DriverError>> {
        self.st().placed.push(("batch", place));
        self.run_batch(handle, connect, place, steps, deadline)
    }

    fn stream_files(
        &self,
        handle: &BoxHandle,
        dir: &BoxPath,
        names: &[String],
        _: Deadline,
        sink: &mut dyn FnMut(&str, Option<&mut dyn std::io::Read>),
    ) -> Result<(), DriverError> {
        self.run_stream(handle, dir, names, sink)
    }

    fn launch_run(&self, h: &BoxHandle, run: i64, spec: &ExecSpec) -> runs::Launch {
        self.local_launch(h, run, spec)
    }

    fn follow_run(
        &self,
        h: &BoxHandle,
        run: i64,
        from: u64,
        out: &mut dyn std::io::Write,
        _: Deadline,
    ) -> Result<Followed, DriverError> {
        self.local_follow(h, run, from, out)
    }

    fn end_run(&self, h: &BoxHandle, run: i64, _: Option<&BoxPath>) -> Result<RunEnd, DriverError> {
        self.local_end(h, run)
    }

    fn cancel_run(&self, h: &BoxHandle, run: i64, grace: u32) -> Result<Cancelled, DriverError> {
        self.local_cancel(h, run, grace)
    }

    fn sleep(&self, h: &BoxHandle) -> Result<SleepOutcome, DriverError> {
        self.local_sleep(h)
    }

    fn wake(&self, handle: &BoxHandle) -> Result<(), DriverError> {
        self.local_wake(handle)
    }
}

mod acquire;
mod files;
mod hooks;
mod knobs;
mod piped;
mod places;
mod ports;
mod runs;
mod script;
mod sleeps;
#[cfg(test)]
mod tests;
mod trees;
