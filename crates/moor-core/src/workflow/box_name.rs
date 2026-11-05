//! The name of the Box a workflow firing starts.

use super::WorkflowId;
use crate::name::{BoxName, ForgeRepo, NameError};
use crate::seam::clock::Ts;

/// The longest part of a Box name.
const PART_MAX: usize = 40;

/// `text` as a Box name part: lowercase letters and digits, other runs becoming one `-`, at most
/// `max` bytes, with no `-` at either end. Empty text gives `x`.
fn slug(text: &str, max: usize) -> String {
    let mut out = String::new();
    for c in text.chars().map(|c| c.to_ascii_lowercase()) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.truncate(max);
    let trimmed = out.trim_end_matches('-');
    if trimmed.is_empty() {
        "x".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// The Box name for workflow `id`, called `workflow`, firing for `repo` at `due_at`.
///
/// It is `<repo>/w<id>-<workflow>-<due as UTC yyyymmddhhmm>`. The id and the due time keep it
/// unique per firing, and the workflow name is cut to fit.
///
/// # Errors
/// [`NameError`] when `due_at` is outside the years the clock can print.
pub fn box_name(
    id: WorkflowId,
    workflow: &str,
    repo: &ForgeRepo,
    due_at: Ts,
) -> Result<BoxName, NameError> {
    let invalid = || NameError::BoxName(format!("{repo}@{}", due_at.as_millis()));
    let stamp = jiff::Timestamp::from_millisecond(due_at.as_millis())
        .map_err(|_| invalid())?
        .strftime("%Y%m%d%H%M")
        .to_string();
    let head = format!("w{id}-");
    let room = PART_MAX.saturating_sub(head.len() + 1 + stamp.len());
    let label = format!("{head}{}-{stamp}", slug(workflow, room));
    BoxName::parse(&format!("{}/{label}", slug(&repo.name, PART_MAX)))
}
