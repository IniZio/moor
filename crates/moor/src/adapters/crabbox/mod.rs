//! Crabbox adapter: the only code that knows crabbox terms.

mod caps;
mod cp;
mod errors;
mod exec;
mod parse;
#[cfg(test)]
mod tests;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::name::{BoxHandle, BoxPath, ProviderId};
use moor_core::seam::driver::{
    AcquireReq, Acquired, BoxListing, Driver, DriverCaps, DriverError, EgressApplied, EgressPlan,
    ExecOutcome, ExecSpec, Released,
};
use moor_core::seam::proc::{Deadline, Proc, ProcOutput, ProcSpec, Tool};

use errors::{map_error, Op};

/// Time allowed for a warmup.
const ACQUIRE_DEADLINE: Duration = Duration::from_mins(15);
/// Time allowed for every other call.
const QUICK_DEADLINE: Duration = Duration::from_secs(60);
/// Provider that takes an allow-list flag.
const FLAG_EGRESS_PROVIDER: &str = "sprites";
/// Provider that takes an allow-list environment variable.
const ENV_EGRESS_PROVIDER: &str = "namespace-devbox";
/// Environment variable carrying the allow-list.
const ENV_EGRESS_VAR: &str = "CRABBOX_NAMESPACE_EGRESS_DOMAINS";
/// Environment variable that makes a release delete rather than pause.
const DELETE_ON_RELEASE: &str = "CRABBOX_NAMESPACE_DELETE_ON_RELEASE";

/// Provider whose Boxes share one slug and need the Box id and port in env.
const STATIC_PROVIDER: &str = "ssh";
/// Port the static provider dials (not configurable yet).
const DEFAULT_SSH_PORT: u16 = 22;

/// Driver backed by the `crabbox` CLI.
pub(crate) struct Crabbox {
    /// Spawner for every crabbox call.
    proc: Arc<dyn Proc>,
}

impl Crabbox {
    /// Wraps a spawner.
    pub(crate) fn new(proc: Arc<dyn Proc>) -> Self {
        Self { proc }
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

/// Per-Task Box name: `moor-{name}-{nonce}`.
fn box_name(req: &AcquireReq) -> String {
    format!("moor-{}-{}", req.name.as_str(), req.nonce)
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

/// Maps a moor size to crabbox's machine class and namespace size.
fn size_of(size: Option<&str>) -> Result<(&'static str, &'static str), DriverError> {
    match size.unwrap_or("small") {
        "small" => Ok(("standard", "S")),
        "medium" => Ok(("fast", "M")),
        "large" => Ok(("large", "L")),
        "xlarge" => Ok(("beast", "XL")),
        other => Err(DriverError::Failed {
            summary: format!("unknown box size {other:?}"),
        }),
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
    let mut args = Vec::new();
    if let Some(p) = provider {
        args.extend(["--provider".to_owned(), p.to_owned()]);
    }
    args.extend(["--id".to_owned(), name.to_owned()]);
    args
}

/// Adds the egress plan to a warmup when the provider can take it.
fn with_egress(spec: ProcSpec, provider: &str, plan: &EgressPlan) -> (ProcSpec, EgressApplied) {
    let EgressPlan::Allow(hosts) = plan else {
        return (spec, EgressApplied::Enforced);
    };
    let list = hosts.join(",");
    match provider {
        FLAG_EGRESS_PROVIDER => (
            spec.args(["--sprites-network-allow", &list]),
            EgressApplied::Enforced,
        ),
        ENV_EGRESS_PROVIDER => (spec.env(ENV_EGRESS_VAR, list), EgressApplied::Enforced),
        _ => (spec, EgressApplied::NotEnforced),
    }
}

/// Builds the workdir from a work root and the Box name.
fn workdir_of(root: &str, name: &str) -> Result<BoxPath, DriverError> {
    BoxPath::under(root, name).map_err(|_| DriverError::Failed {
        summary: "box reported an unusable work root".to_owned(),
    })
}

/// Error for output moor cannot read.
fn unreadable(what: &str) -> DriverError {
    DriverError::Failed {
        summary: format!("unreadable box provider output for {what}"),
    }
}

impl Driver for Crabbox {
    fn caps(&self, provider: &ProviderId) -> Result<DriverCaps, DriverError> {
        let spec = Self::spec(QUICK_DEADLINE).args(["providers", "--json"]);
        let out = self.call(Op::Caps, None, &spec)?;
        caps::caps_from(&out.stdout, provider)
    }

    fn acquire(&self, req: &AcquireReq) -> Result<Acquired, DriverError> {
        let provider = req.provider.as_str();
        let name = box_name(req);
        let (class, ns_size) = size_of(req.class.as_deref())?;
        let mut spec = Self::spec(ACQUIRE_DEADLINE).args(["warmup", "--provider", provider]);
        if let Some(dir) = &req.worktree {
            spec = spec.cwd(dir);
        }
        spec = spec
            .args(["--slug", &name, "--class", class])
            .env("CRABBOX_NAMESPACE_SIZE", ns_size);
        let handle = handle_of(provider, &name);
        let spec = static_env(spec, &handle);
        let (spec, egress) = with_egress(spec, provider, &req.egress);
        let out = self.call(Op::Acquire, Some(provider), &spec)?;
        let workdir = match parse::parse_warmup(&out.stdout).and_then(|w| w.workroot) {
            Some(root) => workdir_of(&root, &name)?,
            None => self.workdir(&handle)?,
        };
        Ok(Acquired {
            handle,
            workdir,
            egress,
        })
    }

    fn planned_handle(&self, req: &AcquireReq) -> BoxHandle {
        handle_of(req.provider.as_str(), &box_name(req))
    }

    fn release(&self, handle: &BoxHandle) -> Result<Released, DriverError> {
        let spec = Self::spec(QUICK_DEADLINE)
            .arg("stop")
            .args(target_args(handle))
            .env(DELETE_ON_RELEASE, "1");
        let spec = static_env(spec, handle);
        match self.call(Op::Release, split_handle(handle).0, &spec) {
            Ok(_) => Ok(Released::Deleted),
            Err(DriverError::NotFound) => Ok(Released::AlreadyGone),
            Err(e) => Err(e),
        }
    }

    fn list(&self, provider: &ProviderId) -> Result<Vec<BoxListing>, DriverError> {
        let p = provider.as_str();
        if p == STATIC_PROVIDER {
            return Err(DriverError::Unsupported("box provider has no inventory"));
        }
        let spec = Self::spec(QUICK_DEADLINE).args(["list", "--provider", p, "--json"]);
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

    fn workdir(&self, handle: &BoxHandle) -> Result<BoxPath, DriverError> {
        let spec = Self::spec(QUICK_DEADLINE)
            .arg("inspect")
            .args(target_args(handle))
            .arg("--json");
        let spec = static_env(spec, handle);
        let out = self.call(Op::Workdir, None, &spec)?;
        let root = parse::parse_inspect_workroot(&out.stdout)
            .map_err(|_| unreadable("workdir"))?
            .ok_or_else(|| unreadable("workdir"))?;
        workdir_of(&root, split_handle(handle).1)
    }

    fn exec(
        &self,
        handle: &BoxHandle,
        spec: &ExecSpec,
        deadline: Deadline,
    ) -> Result<ExecOutcome, DriverError> {
        exec::run(self, handle, spec, deadline)
    }

    fn copy_out(
        &self,
        handle: &BoxHandle,
        from: &BoxPath,
        to: &Path,
        host_cwd: &Path,
    ) -> Result<(), DriverError> {
        cp::copy_out(self, handle, from, to, host_cwd)
    }
}
