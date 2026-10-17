//! The macOS Box layout of a Task, found from its workdir.

use moor_core::task_key::{MacLayout, TaskKey};
use moor_core::TaskId;

use super::Site;
use crate::cx::Cx;
use crate::ops::common::simulator;

/// A Mac Task's layout, key and simulator UDID, when it holds a simulator device.
pub(super) type MacBox = (MacLayout, TaskKey, Option<String>);

/// The macOS Box layout of `task`, its key and its simulator UDID, when the workdir is inside that layout.
///
/// # Errors
/// Returns a message when the store cannot be read for the Task's simulator device.
pub(super) fn mac_layout(
    cx: &Cx,
    site: &Site<'_>,
    task: Option<TaskId>,
) -> Result<Option<MacBox>, String> {
    let (Some(workdir), Some(task)) = (site.workdir, task) else {
        return Ok(None);
    };
    let Some(row) = cx.store().ok().and_then(|s| s.task(task).ok().flatten()) else {
        return Ok(None);
    };
    let key = TaskKey::new(&row.name);
    let Some(layout) = MacLayout::find(workdir, &key) else {
        return Ok(None);
    };
    let store = cx.store().map_err(|e| e.error.to_string())?;
    let sim = simulator::recorded(store, task)
        .map_err(|e| format!("could not read the Task's simulator device: {e}"))?;
    Ok(Some((layout, key, sim)))
}
