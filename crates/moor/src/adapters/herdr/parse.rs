//! Tolerant parsers for herdr's JSON output.

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

/// One Workspace herdr lists.
#[derive(Deserialize)]
pub(super) struct WorkspaceEntry {
    /// Workspace id.
    pub workspace_id: String,
    /// Workspace label.
    pub label: Option<String>,
}

/// `workspace list` result.
#[derive(Deserialize)]
struct WorkspaceList {
    /// Open Workspaces.
    #[serde(default)]
    workspaces: Vec<WorkspaceEntry>,
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

/// Unwraps the `result` envelope into `T`.
fn decode<T: DeserializeOwned>(what: &str, out: &str) -> Result<T, SurfaceError> {
    serde_json::from_str::<Envelope<T>>(out)
        .map(|e| e.result)
        .map_err(|e| SurfaceError::Failed {
            summary: format!("unreadable {what} output: {e}"),
        })
}

/// Parses `workspace list` output.
pub(super) fn workspace_list(out: &str) -> Result<Vec<WorkspaceEntry>, SurfaceError> {
    Ok(decode::<WorkspaceList>("workspace list", out)?.workspaces)
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
