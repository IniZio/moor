//! The host toolchains a repo selects for a macOS Box: grants, env, and the link leases.

use moor_core::config::{select_toolchains, ConfigError, SelectedToolchain, TaskSettings};
use moor_core::seam::engine::MacGrants;
use moor_core::TaskId;

use crate::cx::Cx;
use crate::store::lease::{LeaseId, LeaseKind};
use crate::store::{Store, StoreError};

/// The toolchains `cfg` selects from the host config, in the order the repo names them.
///
/// # Errors
/// Returns a [`ConfigError`] at `box.toolchains[<i>]` for a name the host config does not define.
pub(crate) fn selected(cx: &Cx, cfg: &TaskSettings) -> Result<Vec<SelectedToolchain>, ConfigError> {
    select_toolchains(&cx.config().toolchains, &cfg.repo_box.toolchains)
}

/// What the repo's `box:` section grants the agent's sandbox on a macOS Box.
///
/// # Errors
/// Returns a [`ConfigError`] when `box.toolchains` names a toolchain the host config lacks.
pub(crate) fn mac_grants(cx: &Cx, cfg: &TaskSettings) -> Result<MacGrants, ConfigError> {
    Ok(MacGrants {
        capabilities: cfg.repo_box.capabilities.clone(),
        sandbox: cfg.repo_box.sandbox.clone(),
        toolchains: selected(cx, cfg)?,
    })
}

/// The environment of `toolchains` and their `PATH` directories, each in order.
pub(crate) fn env_and_bin(
    toolchains: &[SelectedToolchain],
) -> (Vec<(String, String)>, Vec<String>) {
    let env = toolchains.iter().flat_map(|t| t.env.clone()).collect();
    let bin = toolchains.iter().flat_map(|t| t.bin.clone()).collect();
    (env, bin)
}

/// Plans one link lease per toolchain, keyed by its root, before the Box creates the links.
///
/// Planning a link already recorded returns its existing lease.
///
/// # Errors
/// A store failure.
pub(crate) fn plan_links(
    store: &Store,
    task: TaskId,
    toolchains: &[SelectedToolchain],
) -> Result<Vec<LeaseId>, StoreError> {
    toolchains
        .iter()
        .map(|t| store.plan_lease(task, LeaseKind::ToolchainLink, &t.root))
        .collect()
}

/// Marks `leases` held once the links exist.
///
/// # Errors
/// A store failure.
pub(crate) fn hold(store: &Store, leases: &[LeaseId]) -> Result<(), StoreError> {
    for lease in leases {
        store.hold_lease(*lease)?;
    }
    Ok(())
}

/// Plans the link leases of an exec on `task`'s Mac Box; touches the store only when there are links.
///
/// # Errors
/// A message when the store fails.
pub(crate) fn plan_for_exec(
    cx: &Cx,
    task: Option<TaskId>,
    toolchains: &[SelectedToolchain],
) -> Result<Vec<LeaseId>, String> {
    let Some(task) = task.filter(|_| !toolchains.is_empty()) else {
        return Ok(Vec::new());
    };
    let store = cx.store().map_err(|e| e.error.to_string())?;
    plan_links(store, task, toolchains).map_err(|e| e.to_string())
}

/// Holds the leases [`plan_for_exec`] planned.
///
/// # Errors
/// A message when the store fails.
pub(crate) fn hold_for_exec(cx: &Cx, leases: &[LeaseId]) -> Result<(), String> {
    if leases.is_empty() {
        return Ok(());
    }
    let store = cx.store().map_err(|e| e.error.to_string())?;
    hold(store, leases).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
