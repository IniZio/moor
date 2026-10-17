//! The ordered Box release steps and which lease rows each step answers for.

use moor_core::boxscript::quote;
use moor_core::task_key::release::{
    kill_tree_script, orphan_ibtoold_script, path_pids_cmd, stop_agent_script, tagged_pids_cmd,
};
use moor_core::task_key::release_sim::{delete_devices_script, remove_root_script, verify_script};
use moor_core::task_key::{MacLayout, TaskKey};

use crate::store::lease::{LeaseId, LeaseKind, LeaseRow};

/// Seconds to wait for the agent to exit after `TERM` before it is killed.
pub(crate) const STOP_AGENT_WAIT_SECS: u32 = 10;

/// One step of releasing a Box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// `TERM` the agent and wait for it to exit.
    StopAgent,
    /// Kill what is left by process group, tag and path.
    KillTree,
    /// Kill an orphaned `ibtoold`.
    OrphanIbtoold,
    /// Delete simulator devices and `CoreSimulator` logs.
    DeleteDevices,
    /// Run the repo teardown.
    Teardown,
    /// Take the compose project down.
    ComposeDown,
    /// Remove the Box root.
    RemoveRoot,
    /// Check nothing of the Box is left.
    Verify,
}

impl Step {
    /// Short plain name used in warnings, e.g. "stop the agent".
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::StopAgent => "stop the agent",
            Self::KillTree => "kill the process tree",
            Self::OrphanIbtoold => "kill the orphaned ibtoold",
            Self::DeleteDevices => "delete the simulator devices",
            Self::Teardown => "run the repo teardown",
            Self::ComposeDown => "take the compose project down",
            Self::RemoveRoot => "remove the Box root",
            Self::Verify => "verify nothing is left",
        }
    }
}

/// What a macOS Box release needs to know.
pub(crate) struct MacTarget<'a> {
    /// The Task key every match derives from.
    pub key: &'a TaskKey,
    /// The Box directories.
    pub layout: &'a MacLayout,
}

/// Whether any row is of `kind`.
fn has_kind(rows: &[LeaseRow], kind: LeaseKind) -> bool {
    rows.iter().any(|r| r.kind == kind)
}

/// Steps in order for a Box; `mac` is None for a non-Mac Box.
pub(crate) fn plan(mac: Option<&MacTarget<'_>>, rows: &[LeaseRow]) -> Vec<Step> {
    let compose = has_kind(rows, LeaseKind::Compose);
    let mut steps = Vec::new();
    if mac.is_some() {
        steps.extend([
            Step::StopAgent,
            Step::KillTree,
            Step::OrphanIbtoold,
            Step::DeleteDevices,
        ]);
    }
    steps.push(Step::Teardown);
    if compose {
        steps.push(Step::ComposeDown);
    }
    if mac.is_some() {
        steps.extend([Step::RemoveRoot, Step::Verify]);
    }
    steps
}

/// The Box script for a step; `None` for `Teardown` (the repo teardown runs it) and for
/// Mac-only steps without a `MacTarget`.
pub(crate) fn script(step: Step, mac: Option<&MacTarget<'_>>, rows: &[LeaseRow]) -> Option<String> {
    match step {
        Step::Teardown => None,
        Step::ComposeDown => rows.iter().find(|r| r.kind == LeaseKind::Compose).map(|r| {
            format!(
                "docker compose -p {} down -v --remove-orphans",
                quote(&r.key)
            )
        }),
        Step::StopAgent => mac.map(|m| stop_agent_script(m.key, m.layout, STOP_AGENT_WAIT_SECS)),
        Step::KillTree => mac.map(|m| {
            let pgids: Vec<i64> = rows
                .iter()
                .filter(|r| r.kind == LeaseKind::Process)
                .filter_map(|r| r.process.map(|p| p.pgid))
                .collect();
            kill_tree_script(m.key, m.layout, &pgids)
        }),
        Step::OrphanIbtoold => mac.map(|m| orphan_ibtoold_script(m.layout)),
        Step::DeleteDevices => mac.map(|m| delete_devices_script(m.layout)),
        Step::RemoveRoot => mac.map(|m| remove_root_script(m.layout)),
        Step::Verify => {
            mac.map(|m| verify_script(m.layout, &tagged_pids_cmd(m.key), &path_pids_cmd(m.layout)))
        }
    }
}

/// Lease rows a step answers for: `Process`->`KillTree`, `SimSet`/`SimDevice`->`DeleteDevices`,
/// `Compose`->`ComposeDown`, `BoxRoot`/`BoxDir`/`Home`/`Xcconfig`/`Cache`->`RemoveRoot`. A step
/// with none of its own answers on the `BoxRoot` row, else the first row, else nothing.
pub(crate) fn rows_for(step: Step, rows: &[LeaseRow]) -> Vec<LeaseId> {
    let own: Vec<LeaseId> = rows
        .iter()
        .filter(|r| owner(r.kind) == step)
        .map(|r| r.id)
        .collect();
    if !own.is_empty() {
        return own;
    }
    rows.iter()
        .find(|r| r.kind == LeaseKind::BoxRoot)
        .or_else(|| rows.first())
        .map(|r| vec![r.id])
        .unwrap_or_default()
}

/// The step that answers for rows of `kind`.
const fn owner(kind: LeaseKind) -> Step {
    match kind {
        LeaseKind::Process => Step::KillTree,
        LeaseKind::SimSet | LeaseKind::SimDevice => Step::DeleteDevices,
        LeaseKind::Compose => Step::ComposeDown,
        LeaseKind::BoxRoot
        | LeaseKind::BoxDir
        | LeaseKind::Home
        | LeaseKind::Xcconfig
        | LeaseKind::Cache => Step::RemoveRoot,
    }
}

#[cfg(test)]
mod tests;
