//! Crabbox adapter: the only code that knows crabbox terms.

mod batch;
mod cache;
mod caps;
mod connect;
mod cp;
mod errors;
mod exec;
mod handle;
mod lease;
mod link;
mod mirror;
mod parse;
mod peek;
mod pin;
mod progress;
mod put;
mod retry;
mod runbox;
mod sleep;
mod stream;
#[cfg(test)]
mod tests;
mod touch;
mod tunnel;
mod version;
mod workdir;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use moor_core::box_key::WORK_ROOT;
use moor_core::name::{BoxHandle, BoxPath, ProviderId};
use moor_core::seam::clock::{Clock, Ts};
use moor_core::seam::driver::{
    AcquireReq, Acquired, BoxListing, BoxStep, Cancelled, Driver, DriverCaps, DriverError,
    ExecOutcome, ExecSpec, Followed, Launched, LocalTunnel, MirrorStats, Released, RunEnd,
    SleepOutcome, StepDone, TunnelHandle,
};
use moor_core::seam::proc::{Deadline, Proc, ProcOutput, ProcSpec, Tool};
use moor_core::seam::rng::Rng;

use caps::size_of;
use errors::{map_error, unreadable, Op};
use handle::{handle_of, slug, split_handle, static_env, target_args};

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
/// Env key only the external provider reads; other providers keep crabbox's own work root.
const WORK_ROOT_KEY: &str = "CRABBOX_EXTERNAL_WORK_ROOT";
/// Provider whose Boxes share one slug and need the Box id and port in env.
const STATIC_PROVIDER: &str = "ssh";
/// Port the static provider dials (not configurable yet).
const DEFAULT_SSH_PORT: u16 = 22;

/// Driver backed by the `crabbox` CLI.
pub(crate) struct Crabbox {
    /// Spawner for every crabbox call.
    proc: Arc<dyn Proc>,
    /// Spawner of the host-key check's `ssh -G` reads: `proc`, unless a test scripts ssh.
    options: Arc<dyn Proc>,
    /// Source of the waits between claim race retries.
    clock: Arc<dyn Clock>,
    /// Source of the per-exec stderr markers.
    rng: Arc<dyn Rng>,
    /// Set once crabbox has passed the version check.
    version_ok: OnceLock<()>,
    /// Whether `crabbox ssh` takes `--no-probe`, asked on the first lookup; may be shared.
    no_probe: Arc<OnceLock<bool>>,
    /// How often a running Box command touches the Box's lease.
    touch_every: Duration,
    /// Lookup deadlines that replace the built-in ones, keyed by provider id.
    lookup_deadlines: BTreeMap<String, Duration>,
    /// When each lease handle was last sent a heartbeat, on `clock`.
    touched: Mutex<HashMap<String, Ts>>,
    /// Resolved ssh words per lease handle, kept until a call on them fails.
    targets: Mutex<HashMap<String, Vec<String>>>,
    /// Where each Box's shared ssh connection lives; none when moor keeps no state.
    links: Option<link::Dirs>,
    /// `<state>/leases`, which holds each lease's dir; none when moor keeps no state.
    leases: Option<PathBuf>,
    /// Keeps a test driver's state dir alive.
    #[cfg(test)]
    scratch: Option<Arc<tempfile::TempDir>>,
}

impl Crabbox {
    /// Wraps a spawner, a clock and a marker source.
    pub(crate) fn new(proc: Arc<dyn Proc>, clock: Arc<dyn Clock>, rng: Arc<dyn Rng>) -> Self {
        Self {
            options: Arc::clone(&proc),
            proc,
            clock,
            rng,
            version_ok: OnceLock::new(),
            no_probe: Arc::default(),
            touch_every: exec::TOUCH_EVERY,
            lookup_deadlines: BTreeMap::new(),
            touched: Mutex::new(HashMap::new()),
            targets: Mutex::new(HashMap::new()),
            links: None,
            leases: None,
            #[cfg(test)]
            scratch: None,
        }
    }

    /// Forgets every cached ssh target of `handle` and ends its shared connection; its
    /// address may change.
    pub(super) fn forget_targets(&self, handle: &BoxHandle) {
        if let Ok(mut map) = self.targets.lock() {
            map.remove(handle.as_str());
        }
        link::close(self, handle);
    }

    /// Sets per-provider ssh lookup deadlines that override the built-in ones.
    pub(crate) fn with_lookup_deadlines(mut self, deadlines: BTreeMap<String, Duration>) -> Self {
        self.lookup_deadlines = deadlines;
        self
    }

    /// Time allowed for an ssh target lookup on `provider`.
    pub(super) fn lookup_deadline(&self, provider: &str) -> Duration {
        let built_in = if provider == "external" {
            EXTERNAL_LOOKUP_DEADLINE
        } else {
            LOOKUP_DEADLINE
        };
        *self.lookup_deadlines.get(provider).unwrap_or(&built_in)
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

    /// Runs one crabbox call, mapping failure through `op`.
    fn call(
        &self,
        op: Op,
        provider: Option<&str>,
        spec: &ProcSpec,
    ) -> Result<ProcOutput, DriverError> {
        self.proc.run(spec).map_err(|e| map_error(op, provider, &e))
    }

    /// Releases whatever a failed first acquire of `handle` started, which also drops its
    /// lease dir; when that release fails the dir stays so a later release can still name
    /// the lease, and the error carries both failures.
    fn undo_acquire(&self, handle: &BoxHandle, failed: DriverError) -> DriverError {
        match self.release(handle) {
            Ok(_) => failed,
            Err(release) => DriverError::Failed {
                summary: format!(
                    "{failed}; releasing the lease it started also failed, so its lease dir stays for a later release: {release}"
                ),
            },
        }
    }

    /// Asks for the relative work root [`WORK_ROOT`], which crabbox joins to the lease's login home.
    fn spec(deadline: Duration) -> ProcSpec {
        ProcSpec::new(Tool::Crabbox, Deadline(deadline)).env(WORK_ROOT_KEY, WORK_ROOT)
    }
}

impl Driver for Crabbox {
    fn caps(&self, provider: &ProviderId) -> Result<DriverCaps, DriverError> {
        self.require_version()?;
        let spec = Self::spec(QUICK_DEADLINE).args(["providers", "--json", "--configured"]);
        let out = self.call(Op::Caps, None, &spec)?;
        caps::caps_from(&out.stdout, provider)
    }

    fn acquire(&self, req: &AcquireReq) -> Result<Acquired, DriverError> {
        self.acquire_noting(req, &mut |_| {})
    }

    fn acquire_noting(
        &self,
        req: &AcquireReq,
        note: &mut dyn FnMut(&str),
    ) -> Result<Acquired, DriverError> {
        workdir::acquire(self, req, note)
    }

    fn planned_handle(&self, req: &AcquireReq) -> BoxHandle {
        handle_of(req.provider.as_str(), &slug(req))
    }

    fn release(&self, handle: &BoxHandle) -> Result<Released, DriverError> {
        self.require_version()?;
        let spec = Self::spec(QUICK_DEADLINE)
            .arg("stop")
            .args(target_args(handle))
            .arg("--delete");
        let spec = self.at_lease(spec, handle)?;
        let result = self.call(Op::Release, split_handle(handle).0, &spec);
        self.forget_targets(handle);
        let released = match result {
            Ok(_) => Released::Deleted,
            Err(DriverError::NotFound) => Released::AlreadyGone,
            Err(e) => return Err(e),
        };
        self.drop_lease_dir(handle);
        Ok(released)
    }

    fn manual_release(&self, handle: &BoxHandle) -> Option<String> {
        self.stop_by_hand(handle)
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
            })
            .collect())
    }

    /// Refused: only crabbox's warmup reports where a Box's workdir is, and moor never
    /// searches the Box for one by name.
    fn workdir(&self, _handle: &BoxHandle) -> Result<BoxPath, DriverError> {
        Err(DriverError::Failed {
            summary: "moor has no recorded workdir for the Box and does not search the Box for one"
                .to_owned(),
        })
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

    fn copy_out(&self, handle: &BoxHandle, from: &BoxPath, to: &Path) -> Result<(), DriverError> {
        self.require_version()?;
        cp::copy_out(self, handle, from, to)
    }

    fn mirror_out(
        &self,
        handle: &BoxHandle,
        from: &BoxPath,
        to: &Path,
        exclude: &[String],
    ) -> Result<MirrorStats, DriverError> {
        self.require_version()?;
        mirror::mirror_out(self, handle, from, to, exclude)
    }

    fn stream_files(
        &self,
        handle: &BoxHandle,
        dir: &BoxPath,
        names: &[String],
        deadline: Deadline,
        sink: &mut dyn FnMut(&str, Option<&mut dyn std::io::Read>),
    ) -> Result<(), DriverError> {
        self.require_version()?;
        stream::stream_files(self, handle, dir, names, deadline, sink)
    }

    fn launch_run(
        &self,
        h: &BoxHandle,
        run: i64,
        spec: &ExecSpec,
    ) -> Result<Launched, DriverError> {
        self.require_version()?;
        runbox::launch(self, h, run, spec)
    }

    fn follow_run(
        &self,
        handle: &BoxHandle,
        run: i64,
        from: u64,
        out: &mut dyn std::io::Write,
        deadline: Deadline,
    ) -> Result<Followed, DriverError> {
        self.require_version()?;
        runbox::follow(self, handle, run, from, out, deadline)
    }

    fn end_run(&self, h: &BoxHandle, run: i64, w: Option<&BoxPath>) -> Result<RunEnd, DriverError> {
        self.require_version()?;
        runbox::end(self, h, run, w)
    }

    fn cancel_run(&self, h: &BoxHandle, run: i64, grace: u32) -> Result<Cancelled, DriverError> {
        self.require_version()?;
        runbox::cancel(self, h, run, grace)
    }

    fn peek_file(&self, handle: &BoxHandle, path: &BoxPath) -> Result<Option<String>, DriverError> {
        self.require_version()?;
        peek::peek_file(self, handle, path)
    }

    fn put_file(
        &self,
        handle: &BoxHandle,
        path: &BoxPath,
        bytes: &[u8],
        mode: u32,
    ) -> Result<(), DriverError> {
        self.require_version()?;
        put::put_file(self, handle, path, bytes, mode)
    }

    fn batch(
        &self,
        handle: &BoxHandle,
        connect: &[String],
        steps: &[BoxStep],
        deadline: Deadline,
    ) -> Vec<Result<StepDone, DriverError>> {
        if let Err(e) = self.require_version() {
            return vec![Err(e)];
        }
        batch::run(self, handle, connect, steps, deadline)
    }

    fn connect_argv(&self, handle: &BoxHandle) -> Result<Vec<String>, DriverError> {
        self.require_version()?;
        connect::connect_argv(self, handle)
    }

    fn sleep(&self, handle: &BoxHandle) -> Result<SleepOutcome, DriverError> {
        let result = sleep::sleep(self, handle);
        self.forget_targets(handle);
        result
    }

    fn wake(&self, handle: &BoxHandle) -> Result<(), DriverError> {
        let result = sleep::wake(self, handle);
        self.forget_targets(handle);
        result.and_then(|()| touch::heartbeat(self, handle))
    }

    fn touch(&self, handle: &BoxHandle) -> Result<(), DriverError> {
        touch::heartbeat(self, handle)
    }

    fn reverse_tunnel(
        &self,
        handle: &BoxHandle,
        box_port: u16,
        socket: &Path,
    ) -> Result<Box<dyn TunnelHandle>, DriverError> {
        self.require_version()?;
        tunnel::reverse_tunnel(self, handle, box_port, socket)
    }

    fn local_tunnel(&self, handle: &BoxHandle, box_port: u16) -> Result<LocalTunnel, DriverError> {
        self.require_version()?;
        tunnel::local_tunnel(self, handle, box_port)
    }
}
