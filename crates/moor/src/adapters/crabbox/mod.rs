//! Crabbox adapter: the only code that knows crabbox terms.

mod batch;
mod caps;
mod connect;
mod cp;
mod errors;
mod exec;
mod mac;
mod names;
mod parse;
mod peek;
mod put;
mod sleep;
#[cfg(test)]
mod tests;
mod tunnel;
mod version;
mod workdir;

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use moor_core::name::{BoxHandle, BoxPath, ProviderId};
use moor_core::seam::clock::Clock;
use moor_core::seam::driver::{
    AcquireReq, Acquired, BoxListing, BoxStep, Driver, DriverCaps, DriverError, ExecOutcome,
    ExecSpec, Platform, Released, SleepOutcome, StepDone, TunnelHandle,
};
use moor_core::seam::proc::{Deadline, Proc, ProcError, ProcOutput, ProcSpec, Tool};

use errors::{is_claim_race, map_error, Op};
use names::{box_name, handle_of, size_of, split_handle, target_args, unreadable};

/// Time allowed for a warmup.
const ACQUIRE_DEADLINE: Duration = Duration::from_mins(15);
/// Time allowed for a provider inventory listing.
const LIST_DEADLINE: Duration = Duration::from_secs(5);
/// Time allowed for every other call.
const QUICK_DEADLINE: Duration = Duration::from_secs(60);
/// Time allowed for an ssh target lookup unless the provider has its own.
const LOOKUP_DEADLINE: Duration = Duration::from_secs(5);
/// An external provider's ssh lookup runs through the adapter and was measured at ~6 s.
const EXTERNAL_LOOKUP_DEADLINE: Duration = Duration::from_secs(20);
/// Provider whose Boxes share one slug and need the Box id and port in env.
const STATIC_PROVIDER: &str = "ssh";
/// Port the static provider dials (not configurable yet).
const DEFAULT_SSH_PORT: u16 = 22;

/// Driver backed by the `crabbox` CLI.
pub(crate) struct Crabbox {
    /// Spawner for every crabbox call.
    proc: Arc<dyn Proc>,
    /// Source of the waits between claim race retries.
    clock: Arc<dyn Clock>,
    /// Set once crabbox has passed the version check.
    version_ok: OnceLock<()>,
    /// Lookup deadlines that replace the built-in ones, keyed by provider id.
    lookup_deadlines: BTreeMap<String, Duration>,
    /// Resolved ssh words per (lease handle, host cwd), kept for this driver's lifetime.
    targets: Mutex<HashMap<(String, PathBuf), Vec<String>>>,
    /// The `CRABBOX_TARGET` crabbox runs with.
    target: Option<String>,
    /// The `CRABBOX_STATIC_WORK_ROOT` crabbox runs with.
    work_root: Option<String>,
}

impl Crabbox {
    /// Wraps a spawner.
    pub(crate) fn new(proc: Arc<dyn Proc>, clock: Arc<dyn Clock>) -> Self {
        Self {
            proc,
            clock,
            version_ok: OnceLock::new(),
            lookup_deadlines: BTreeMap::new(),
            targets: Mutex::new(HashMap::new()),
            target: None,
            work_root: None,
        }
    }

    /// Records the `CRABBOX_TARGET` and `CRABBOX_STATIC_WORK_ROOT` crabbox runs with.
    pub(crate) fn with_target(mut self, target: Option<String>, work_root: Option<String>) -> Self {
        self.target = target;
        self.work_root = work_root;
        self
    }

    /// The platform of the Boxes crabbox is configured for.
    fn platform(&self) -> Platform {
        Platform::from_target(self.target.as_deref())
    }

    /// Forgets every cached ssh target of `handle`; its address may change.
    pub(super) fn forget_targets(&self, handle: &BoxHandle) {
        if let Ok(mut map) = self.targets.lock() {
            map.retain(|(h, _), _| h != handle.as_str());
        }
    }

    /// Sets per-provider ssh lookup deadlines that override the built-in ones.
    pub(crate) fn with_lookup_deadlines(mut self, deadlines: BTreeMap<String, Duration>) -> Self {
        self.lookup_deadlines = deadlines;
        self
    }

    /// Time allowed for an ssh target lookup on `provider`.
    pub(super) fn lookup_deadline(&self, provider: &str) -> Duration {
        self.lookup_deadlines
            .get(provider)
            .copied()
            .unwrap_or(if provider == "external" {
                EXTERNAL_LOOKUP_DEADLINE
            } else {
                LOOKUP_DEADLINE
            })
    }

    /// Refuses a crabbox older than moor supports; asks once and remembers a pass.
    fn require_version(&self) -> Result<(), DriverError> {
        if self.version_ok.get().is_some() {
            return Ok(());
        }
        let spec = Self::spec(QUICK_DEADLINE).arg("--version");
        let out = self.call(Op::Caps, None, &spec)?;
        version::check(&out.stdout)?;
        let _ = self.version_ok.set(());
        Ok(())
    }

    /// A driver whose waits advance a fake clock and whose version check is already passed.
    #[cfg(test)]
    pub(crate) fn fake(proc: Arc<dyn Proc>) -> Self {
        let driver = Self::fake_unchecked(proc);
        let _ = driver.version_ok.set(());
        driver
    }

    /// Like [`Self::fake`], but still asks crabbox for its version.
    #[cfg(test)]
    pub(crate) fn fake_unchecked(proc: Arc<dyn Proc>) -> Self {
        use crate::testing::clock::FakeClock;
        use moor_core::seam::clock::Ts;
        Self::new(proc, Arc::new(FakeClock::new(Ts::from_millis(0))))
    }

    /// Runs `call`, trying again while crabbox reports a claim race, for up to `CLAIM_RACE_BUDGET` of waiting.
    fn retry_claim_race<T>(
        &self,
        mut call: impl FnMut() -> Result<T, ProcError>,
    ) -> Result<T, ProcError> {
        let mut waited = Duration::ZERO;
        loop {
            match call() {
                Err(e) if waited + CLAIM_RACE_PAUSE <= CLAIM_RACE_BUDGET && is_claim_race(&e) => {
                    self.clock.sleep(CLAIM_RACE_PAUSE);
                    waited += CLAIM_RACE_PAUSE;
                }
                result => return result,
            }
        }
    }

    /// Runs one crabbox call, mapping failure through `op`.
    fn call(
        &self,
        op: Op,
        provider: Option<&str>,
        spec: &ProcSpec,
    ) -> Result<ProcOutput, DriverError> {
        self.proc.run(spec).map_err(|e| map_error(op, provider, &e))
    }

    /// Starts a crabbox spec.
    fn spec(deadline: Duration) -> ProcSpec {
        ProcSpec::new(Tool::Crabbox, Deadline(deadline))
    }
}

/// Total time a call waits out claim races; a restarting tunnel rewrites the claim every few seconds.
const CLAIM_RACE_BUDGET: Duration = Duration::from_secs(15);
/// Pause between retries of a claim race.
const CLAIM_RACE_PAUSE: Duration = Duration::from_secs(1);

/// Adds the static provider's Box id and port to a call on `handle`.
fn static_env(spec: ProcSpec, handle: &BoxHandle) -> ProcSpec {
    match split_handle(handle) {
        (Some(STATIC_PROVIDER), name) => spec
            .env("CRABBOX_STATIC_ID", name)
            .env("CRABBOX_STATIC_PORT", DEFAULT_SSH_PORT.to_string()),
        _ => spec,
    }
}

impl Driver for Crabbox {
    fn caps(&self, provider: &ProviderId) -> Result<DriverCaps, DriverError> {
        self.require_version()?;
        let spec = Self::spec(QUICK_DEADLINE).args(["providers", "--json"]);
        let out = self.call(Op::Caps, None, &spec)?;
        caps::caps_from(&out.stdout, provider)
    }

    fn acquire(&self, req: &AcquireReq) -> Result<Acquired, DriverError> {
        self.require_version()?;
        let provider = req.provider.as_str();
        let name = box_name(req);
        let class = size_of(req.class.as_deref())?;
        let layout = self.acquire_layout(req)?;
        let mut spec = Self::spec(ACQUIRE_DEADLINE).args(["warmup", "--provider", provider]);
        if let Some(dir) = &req.worktree {
            spec = spec.cwd(dir);
        }
        spec = spec.args(["--slug", &name, "--class", class]);
        if let Some(l) = &layout {
            spec = spec.env("CRABBOX_STATIC_WORK_ROOT", l.work.as_str());
        }
        let handle = handle_of(provider, &name);
        let spec = static_env(spec, &handle);
        self.forget_targets(&handle);
        let out = self.call(Op::Acquire, Some(provider), &spec)?;
        let warm = parse::parse_warmup(&out.stdout).ok_or_else(|| unreadable("acquire"))?;
        let workdir = self.acquired_workdir(&handle, layout.as_ref(), &warm, req)?;
        Ok(Acquired {
            handle,
            workdir,
            platform: self.platform(),
        })
    }

    fn planned_handle(&self, req: &AcquireReq) -> BoxHandle {
        handle_of(req.provider.as_str(), &box_name(req))
    }

    fn release(&self, handle: &BoxHandle) -> Result<Released, DriverError> {
        self.require_version()?;
        let spec = Self::spec(QUICK_DEADLINE)
            .arg("stop")
            .args(target_args(handle))
            .arg("--delete");
        let spec = static_env(spec, handle);
        let result = self.call(Op::Release, split_handle(handle).0, &spec);
        self.forget_targets(handle);
        match result {
            Ok(_) => Ok(Released::Deleted),
            Err(DriverError::NotFound) => Ok(Released::AlreadyGone),
            Err(e) => Err(e),
        }
    }

    fn list(&self, provider: &ProviderId) -> Result<Vec<BoxListing>, DriverError> {
        self.require_version()?;
        let p = provider.as_str();
        if p == STATIC_PROVIDER {
            return Err(DriverError::Unsupported("box provider has no inventory"));
        }
        let spec = Self::spec(LIST_DEADLINE).args(["list", "--provider", p, "--json"]);
        let out = self.call(Op::List, None, &spec)?;
        let items = parse::parse_list(&out.stdout).map_err(|_| unreadable("list"))?;
        Ok(items
            .into_iter()
            .map(|i| BoxListing {
                handle: handle_of(p, &i.name),
                provider: Some(provider.clone()),
                asleep: i.asleep,
                platform: self.platform(),
            })
            .collect())
    }

    fn workdir(&self, handle: &BoxHandle, worktree: &Path) -> Result<BoxPath, DriverError> {
        self.require_version()?;
        workdir::probe(self, handle, Some(worktree))
    }

    fn exec(
        &self,
        handle: &BoxHandle,
        spec: &ExecSpec,
        deadline: Deadline,
    ) -> Result<ExecOutcome, DriverError> {
        self.require_version()?;
        exec::run(self, handle, spec, deadline)
    }

    fn copy_out(
        &self,
        handle: &BoxHandle,
        from: &BoxPath,
        to: &Path,
        host_cwd: &Path,
    ) -> Result<(), DriverError> {
        self.require_version()?;
        cp::copy_out(self, handle, from, to, host_cwd)
    }

    fn peek_file(
        &self,
        handle: &BoxHandle,
        path: &BoxPath,
        host_cwd: &Path,
    ) -> Result<Option<String>, DriverError> {
        self.require_version()?;
        peek::peek_file(self, handle, path, host_cwd)
    }

    fn put_file(
        &self,
        handle: &BoxHandle,
        path: &BoxPath,
        bytes: &[u8],
        mode: u32,
        host_cwd: &Path,
    ) -> Result<(), DriverError> {
        self.require_version()?;
        put::put_file(self, handle, path, bytes, mode, host_cwd)
    }

    fn batch(
        &self,
        handle: &BoxHandle,
        connect: &[String],
        steps: &[BoxStep],
        host_cwd: &Path,
        deadline: Deadline,
    ) -> Vec<Result<StepDone, DriverError>> {
        if let Err(e) = self.require_version() {
            return vec![Err(e)];
        }
        batch::run(self, handle, connect, steps, host_cwd, deadline)
    }

    fn connect_argv(
        &self,
        handle: &BoxHandle,
        host_cwd: &Path,
    ) -> Result<Vec<String>, DriverError> {
        self.require_version()?;
        connect::connect_argv(self, handle, host_cwd)
    }

    fn sleep(&self, handle: &BoxHandle) -> Result<SleepOutcome, DriverError> {
        let result = sleep::sleep(self, handle);
        self.forget_targets(handle);
        result
    }

    fn wake(&self, handle: &BoxHandle) -> Result<(), DriverError> {
        let result = sleep::wake(self, handle);
        self.forget_targets(handle);
        result
    }

    fn reverse_tunnel(
        &self,
        handle: &BoxHandle,
        box_port: u16,
        host_port: u16,
        host_cwd: &Path,
    ) -> Result<Box<dyn TunnelHandle>, DriverError> {
        self.require_version()?;
        tunnel::reverse_tunnel(self, handle, box_port, host_port, host_cwd)
    }
}
