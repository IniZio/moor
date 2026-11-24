//! The work Boxes a question's place started, as the task page reads them: each with its repo,
//! when it was ready and whether it still runs, and the files it left, served under its own name.

use moor_core::view::ArtifactView;
use serde::Deserialize;
use serde_json::{json, Value};

use super::evidence;

/// A work Box as `show` sends it on its place, the parts a page reads.
#[derive(Deserialize)]
pub(super) struct TaskBox {
    /// The work Box's name.
    name: String,
    /// The repo it was started on, as `OWNER/NAME`.
    repo: String,
    /// RFC 3339 UTC time it was started.
    started_at: String,
    /// Milliseconds from start to ready; none before it is ready.
    #[serde(default)]
    ready_ms: Option<u64>,
    /// Where it stands, as `show` names it.
    state: String,
    /// RFC 3339 UTC time it closed, once it has.
    #[serde(default)]
    closed_at: Option<String>,
    /// The latest version of each file it left.
    #[serde(default)]
    artifacts: Vec<ArtifactView>,
}

/// The page's view of `boxes`, oldest first as `show` sent them; each file's link names the work Box.
pub(super) fn shown(boxes: &[TaskBox]) -> Value {
    let boxes: Vec<Value> = boxes
        .iter()
        .map(|b| {
            let artifacts: Vec<Value> = b.artifacts.iter().map(|a| evidence(&b.name, a)).collect();
            json!({"name": b.name, "repo": b.repo, "started_at": b.started_at,
                   "ready_ms": b.ready_ms, "state": b.state, "closed_at": b.closed_at,
                   "artifacts": artifacts})
        })
        .collect();
    json!(boxes)
}
