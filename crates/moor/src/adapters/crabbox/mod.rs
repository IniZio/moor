//! Crabbox adapter: the only code that knows crabbox terms.

mod batch;
mod caps;
mod connect;
mod cp;
mod errors;
mod exec;
mod lease;
mod link;
mod parse;
mod peek;
mod pin;
mod put;
mod retry;
mod runbox;
mod sleep;
mod stream;
#[cfg(test)]
mod tests;
mod tunnel;
mod version;
mod workdir;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use moor_core::box_key::BoxKey;
use moor_core::name::{BoxHandle, BoxPath, ProviderId};
use moor_core::seam::driver::{
    AcquireReq, Acquired, BoxListing, BoxStep, Cancelled, Driver, DriverCaps, DriverError,
    ExecOutcome, ExecSpec, Followed, Launched, Released, RunEnd, SleepOutcome, StepDone,
    TunnelHandle,
};
use moor_core::seam::proc::{Deadline, Proc, ProcOutput, ProcSpec, Tool};
use moor_core::seam::{clock::Clock, rng::Rng};

use caps::size_of;
use errors::{map_error, Op};

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
    /// Resolved ssh words per lease handle, kept for this driver's lifetime.
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

    /// Asks for the relative work root `moor`, which crabbox joins to the lease's login home.
    fn spec(deadline: Duration) -> ProcSpec {
        ProcSpec::new(Tool::Crabbox, Deadline(deadline)).env(WORK_ROOT_KEY, "moor")
    }
}

/// Per-Box crabbox slug, `moor-{id}-{nonce}`; the Box name never enters it.
fn slug(req: &AcquireReq) -> String {
    BoxKey::new(req.box_id).slug(&req.nonce)
}

/// The handle for a Box named `name` under `provider`.
fn handle_of(provider: &str, name: &str) -> BoxHandle {
    BoxHandle::new(format!("{provider}/{name}"))
}

/// Adds the static provider's Box id and port to a call on `handle`.
fn static_env(spec: ProcSpec, handle: &BoxHandle) -> ProcSpec {
    match split_handle(handle) {
        (Some(STATIC_PROVIDER), name) => spec
            .env("CRABBOX_STATIC_ID", name)
            .env("CRABBOX_STATIC_PORT", DEFAULT_SSH_PORT.to_string()),
        _ => spec,
    }
}

/// Splits a handle into optional provider and Box name.
fn split_handle(handle: &BoxHandle) -> (Option<&str>, &str) {
    match handle.as_str().split_once('/') {
        Some((p, n)) if !p.is_empty() => (Some(p), n),
        Some((_, n)) => (None, n),
        None => (None, handle.as_str()),
    }
}

/// Selector arguments that name one Box.
fn target_args(handle: &BoxHandle) -> Vec<String> {
    let (provider, name) = split_handle(handle);
    let provider = provider.into_iter().flat_map(|p| ["--provider", p]);
    provider.chain(["--id", name]).map(str::to_owned).collect()
}

/// Error for output moor cannot read.
fn unreadable(what: &str) -> DriverError {
    DriverError::Failed {
        summary: format!("unreadable box provider output for {what}"),
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
        self.require_version()?;
        let provider = req.provider.as_str();
        let name = slug(req);
        let class = size_of(req.class.as_deref())?;
        let spec = Self::spec(ACQUIRE_DEADLINE)
            .args(["warmup", "--provider", provider])
            .args(["--slug", &name, "--class", class]);
        let handle = handle_of(provider, &name);
        let existed = self.lease_dir(&handle)?.exists();
        let spec = self.at_lease(spec, &handle)?;
        self.forget_targets(&handle);
        let acquired = workdir::warm_up(self, provider, &handle, &spec);
        if acquired.is_err() && !existed {
            self.drop_lease_dir(&handle);
        }
        acquired
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
        result
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
}
