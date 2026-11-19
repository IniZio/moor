//! The name of the Box a workflow firing starts.

use super::{RunTrigger, WorkflowId};
use crate::name::{BoxName, NameError};
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

/// The Box name for workflow `id`, called `workflow`, firing at `due_at`.
///
/// It is `<workflow>/w<id>-<due as UTC yyyymmddhhmm>`, the workflow name cut to fit. The id and
/// the due time keep it unique per firing, and no repo is in it, so editing the workflow's repos
/// never changes the name of a run already claimed. A manual firing takes the clock to the second
/// and an `m` before it, so it never shares a name with a cron firing due in the same minute.
///
/// # Errors
/// [`NameError`] when `due_at` is outside the years the clock can print.
pub fn box_name(
    id: WorkflowId,
    workflow: &str,
    due_at: Ts,
    trigger: RunTrigger,
) -> Result<BoxName, NameError> {
    let invalid = || NameError::BoxName(format!("{workflow}@{}", due_at.as_millis()));
    let (prefix, format) = match trigger {
        RunTrigger::Cron => ("", "%Y%m%d%H%M"),
        RunTrigger::Manual => ("m", "%Y%m%d%H%M%S"),
    };
    let stamp = jiff::Timestamp::from_millisecond(due_at.as_millis())
        .map_err(|_| invalid())?
        .strftime(format)
        .to_string();
    BoxName::parse(&format!(
        "{}/w{id}-{prefix}{stamp}",
        slug(workflow, PART_MAX)
    ))
}
