//! The per-Task iOS simulator, created on the Mac host once the Box is acquired.

use std::path::{Path, PathBuf};
use std::time::Duration;

use moor_core::config::{BoxCapability, ConfigError, TaskSettings};
use moor_core::domain::run::SyncMode;
use moor_core::seam::driver::{Acquired, ExecOutcome, ExecSpec};
use moor_core::seam::proc::{Deadline, Output};
use moor_core::simulator::{self, Choice};
use moor_core::task_key::{MacLayout, TaskKey};
use moor_core::{BoxHandle, TaskId};

use crate::cx::Cx;
use crate::store::lease::{LeaseKind, LeaseState};
use crate::store::{Store, StoreError};

/// Wall-clock limit of each host step: listing, directory creation and device creation.
const STEP_DEADLINE: Duration = Duration::from_secs(120);

/// First marker line of a framed host step's output.
const BEGIN: &str = "MOOR_SIM_BEGIN";

/// Last marker line of a framed host step's output.
const END: &str = "MOOR_SIM_END";

/// What lies between the marker lines for `nonce`; `None` when either is missing.
fn between_markers(log: &str, nonce: &str) -> Option<String> {
    let (begin, end) = (format!("{BEGIN} {nonce}"), format!("{END} {nonce}"));
    let mut lines = log.lines().skip_while(|l| *l != begin).skip(1);
    let mut out = String::new();
    for l in lines.by_ref() {
        if l == end {
            return Some(out);
        }
        out.push_str(l);
        out.push('\n');
    }
    None
}

/// The UDID of the Task's simulator, once its device lease is held.
///
/// # Errors
/// The lease rows cannot be read.
pub(crate) fn recorded(store: &Store, task: TaskId) -> Result<Option<String>, StoreError> {
    Ok(store
        .leases(task)?
        .into_iter()
        .find(|r| r.kind == LeaseKind::SimDevice && r.state == LeaseState::Held)
        .map(|r| r.key))
}

/// Creates the Task's simulator in its own device set when the repo selects `ios-simulator`.
///
/// Does nothing when the capability is not selected, the Box is not a Mac layout, or a
/// device is already recorded. The steps run on the Mac host outside the sandbox, in
/// order: the set row is recorded as `planned`, the set directory is made, the inventory
/// is read and the pins applied, the device is created, then the device row is recorded and
/// held and the set row is held. The set row exists before anything is created, so a crash
/// between creating the device and recording its UDID still leaves a row whose release
/// deletes everything in the set. The device is not booted.
///
/// # Errors
/// Returns the text to report: a pin that matches nothing on this Mac as a `.moor.yaml`
/// config error, or the step that failed.
pub(crate) fn ensure(
    cx: &Cx,
    store: &Store,
    id: TaskId,
    got: &Acquired,
    worktree: &Path,
    cfg: &TaskSettings,
) -> Result<(), String> {
    if !cfg
        .repo_box
        .capabilities
        .contains(&BoxCapability::IosSimulator)
    {
        return Ok(());
    }
    let Some(row) = store.task(id).map_err(|e| e.to_string())? else {
        return Ok(());
    };
    let key = TaskKey::new(&row.name);
    let Some(layout) = MacLayout::find(&got.workdir, &key) else {
        return Ok(());
    };
    if recorded(store, id).map_err(|e| e.to_string())?.is_some() {
        return Ok(());
    }
    let set = store
        .plan_lease(id, LeaseKind::SimSet, layout.sim.as_str())
        .map_err(|e| e.to_string())?;
    let host = Host::new(cx, id, got, worktree)?;
    let udid = create(&host, cfg, &layout, &key)?;
    let device = store
        .plan_lease(id, LeaseKind::SimDevice, &udid)
        .map_err(|e| e.to_string())?;
    [device, set]
        .into_iter()
        .try_for_each(|lease| store.hold_lease(lease).map(drop))
        .map_err(|e| e.to_string())
}

/// Makes the set directory and the device inside it, and returns the new UDID.
fn create(
    host: &Host<'_>,
    cfg: &TaskSettings,
    layout: &MacLayout,
    key: &TaskKey,
) -> Result<String, String> {
    host.run(
        &["mkdir", "-p", layout.sim.as_str()],
        "make the simulator set directory",
    )?;
    let listing = host.framed(simulator::LIST_SCRIPT, "list the simulators")?;
    let choice = choose(cfg, &listing)?;
    let script = simulator::create_script(&layout.sim, &simulator::device_name(key), &choice);
    let created = host.framed(&script, "create the simulator")?;
    simulator::parse_udid(&created)
        .ok_or_else(|| "could not create the simulator: no UDID in its output".to_owned())
}

/// Applies the repo's pins to the inventory `listing` printed.
fn choose(cfg: &TaskSettings, listing: &str) -> Result<Choice, String> {
    let inventory = simulator::parse_inventory(listing)
        .map_err(|e| format!("could not list the simulators: {e}"))?;
    let pin = cfg.repo_box.simulator.as_ref();
    simulator::choose(
        &inventory,
        pin.and_then(|p| p.device_type.as_deref()),
        pin.and_then(|p| p.runtime.as_deref()),
    )
    .map_err(|e| {
        ConfigError::new(e.path(), e.reason())
            .in_file(".moor.yaml")
            .to_string()
    })
}

/// Where host steps run: the Box's driver, with stdout captured to a host log.
struct Host<'a> {
    /// The context holding the driver.
    cx: &'a Cx,
    /// The acquired Box the steps go through.
    handle: &'a BoxHandle,
    /// Worktree on the host the driver runs from.
    worktree: &'a Path,
    /// File the stdout of a step is captured to.
    log: PathBuf,
}

impl<'a> Host<'a> {
    /// The host steps of Task `id` in the Box `got`, logging under the state directory.
    fn new(cx: &'a Cx, id: TaskId, got: &'a Acquired, worktree: &'a Path) -> Result<Self, String> {
        let log = cx
            .state_dir()
            .map_err(|e| e.error.to_string())?
            .join("simulator")
            .join(format!("{}.log", id.get()));
        Ok(Self {
            cx,
            handle: &got.handle,
            worktree,
            log,
        })
    }

    /// Runs `script` under `sh -c` and returns only its stdout, found between two nonce
    /// marker lines: the driver's log also carries its own banner and summary around the
    /// command output. Without both markers the step failed; `step` names it.
    fn framed(&self, script: &str, step: &str) -> Result<String, String> {
        let nonce = format!("{:08x}", self.cx.rng().u32());
        let wrapped =
            format!("echo {BEGIN} {nonce}; ({script}); c=$?; echo; echo {END} {nonce}; exit $c");
        let log = self.run(&["sh", "-c", &wrapped], step)?;
        between_markers(&log, &nonce)
            .ok_or_else(|| format!("could not {step}: the output had no framed result"))
    }

    /// Runs `argv` and returns its whole captured log; `step` names what failed.
    fn run(&self, argv: &[&str], step: &str) -> Result<String, String> {
        if let Some(dir) = self.log.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("could not {step}: {e}"))?;
        }
        std::fs::File::create(&self.log).map_err(|e| format!("could not {step}: {e}"))?;
        let spec = ExecSpec {
            argv: argv.iter().map(|s| (*s).to_owned()).collect(),
            env: Vec::new(),
            cwd: None,
            sync: SyncMode::None,
            output: Some(Output::Log(self.log.clone())),
            host_cwd: Some(self.worktree.to_path_buf()),
            class: None,
        };
        let result = self
            .cx
            .driver()
            .exec(self.handle, &spec, Deadline(STEP_DEADLINE));
        let out = std::fs::read_to_string(&self.log).unwrap_or_default();
        let _ = std::fs::remove_file(&self.log);
        match result {
            Ok(ExecOutcome::Exited { code: 0 }) => Ok(out),
            Ok(other) => Err(format!("could not {step}: {other:?}")),
            Err(e) => Err(format!("could not {step}: {e}")),
        }
    }
}
