//! Tolerant parsers for herdr's JSON output.

use std::path::PathBuf;

use moor_core::seam::surface::SurfaceError;
use serde::de::DeserializeOwned;
use serde::Deserialize;

/// The `{"result": ...}` wrapper.
#[derive(Deserialize)]
struct Envelope<T> {
    /// Payload.
    result: T,
}

/// A workspace reference.
#[derive(Deserialize)]
struct WsId {
    /// Workspace id.
    workspace_id: String,
}

/// A pane reference.
#[derive(Deserialize)]
struct PaneId {
    /// Pane id.
    pane_id: String,
}

/// One worktree herdr knows about.
#[derive(Deserialize)]
pub(super) struct WorktreeEntry {
    /// Checkout path.
    pub path: Option<PathBuf>,
    /// Checked-out branch.
    pub branch: Option<String>,
    /// Workspace currently open on the worktree.
    pub open_workspace_id: Option<String>,
    /// Whether the checkout is a linked worktree rather than the main one.
    pub is_linked_worktree: Option<bool>,
}

/// `worktree create` result.
#[derive(Deserialize)]
struct WorktreeCreated {
    /// Workspace opened on the worktree.
    workspace: Option<WsId>,
    /// The new worktree.
    worktree: WorktreeEntry,
}

/// `worktree list` result.
#[derive(Deserialize)]
struct WorktreeList {
    /// Known worktrees.
    #[serde(default)]
    worktrees: Vec<WorktreeEntry>,
}

/// `workspace create` result.
#[derive(Deserialize)]
struct WorkspaceCreated {
    /// The new workspace.
    workspace: WsId,
}

/// `tab create` result.
#[derive(Deserialize)]
struct TabCreated {
    /// The tab's first pane.
    root_pane: PaneId,
}

/// A freshly created worktree.
pub(super) struct Created {
    /// Workspace opened on it, when herdr reports one.
    pub workspace_id: Option<String>,
    /// The worktree itself.
    pub entry: WorktreeEntry,
}

/// Unwraps the `result` envelope into `T`.
fn decode<T: DeserializeOwned>(what: &str, out: &str) -> Result<T, SurfaceError> {
    serde_json::from_str::<Envelope<T>>(out)
        .map(|e| e.result)
        .map_err(|e| SurfaceError::Failed {
            summary: format!("unreadable {what} output: {e}"),
        })
}

/// Parses `worktree create` output.
pub(super) fn worktree_created(out: &str) -> Result<Created, SurfaceError> {
    let c: WorktreeCreated = decode("worktree create", out)?;
    let workspace_id = c
        .workspace
        .map(|w| w.workspace_id)
        .or_else(|| c.worktree.open_workspace_id.clone());
    Ok(Created {
        workspace_id,
        entry: c.worktree,
    })
}

/// Parses `worktree list` output.
pub(super) fn worktree_list(out: &str) -> Result<Vec<WorktreeEntry>, SurfaceError> {
    Ok(decode::<WorktreeList>("worktree list", out)?.worktrees)
}

/// Parses `workspace create` output into the workspace id.
pub(super) fn workspace_created(out: &str) -> Result<String, SurfaceError> {
    Ok(decode::<WorkspaceCreated>("workspace create", out)?
        .workspace
        .workspace_id)
}

/// Parses `tab create` output into the root pane id.
pub(super) fn tab_created(out: &str) -> Result<String, SurfaceError> {
    Ok(decode::<TabCreated>("tab create", out)?.root_pane.pane_id)
}
