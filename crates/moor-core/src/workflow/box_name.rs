//! The name of the Box a workflow firing starts.

use super::{PromptHash, RunCause, WorkflowId};
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

/// The name of the Box that makes workflow `id`'s plan for the prompt hashed `prompt`.
///
/// It is `<workflow>/w<id>-plan-<first 8 of the hash>-<at, UTC to the second>`. The stamp keeps a
/// second try for the same prompt clear of the first.
///
/// # Errors
/// [`NameError`] when `at` is outside the years the clock can print, or the name is too long.
pub fn plan_box_name(
    id: WorkflowId,
    workflow: &str,
    prompt: &PromptHash,
    at: Ts,
) -> Result<BoxName, NameError> {
    let stamp = jiff::Timestamp::from_millisecond(at.as_millis())
        .map_err(|_| NameError::BoxName(format!("{workflow}@{}", at.as_millis())))?
        .strftime("%Y%m%d%H%M%S")
        .to_string();
    let hash = prompt.hex();
    let short = hash.get(..8).unwrap_or(&hash);
    BoxName::parse(&format!(
        "{}/w{id}-plan-{short}-{stamp}",
        slug(workflow, PART_MAX)
    ))
}

/// The Box name for workflow `id`, called `workflow`, firing at `due_at` for `cause`.
///
/// It is `<workflow>/w<id>-<due as UTC yyyymmddhhmm>`, the workflow name cut to fit. The id and
/// the due time keep it unique per firing, and no repo is in it, so editing the workflow's repos
/// never changes the name of a run already claimed. A manual firing takes the clock to the second
/// and an `m` before it, so it never shares a name with a cron firing due in the same minute.
///
/// A run for a pull request names it in place of the due time: `w<id>-pr<number>-<first 7 of the
/// head SHA>`. That is unique per pull request and head, as the claim is. A manual run for a pull
/// request adds its `m` stamp after, so Run again at an unchanged head never meets the name of an
/// earlier run's Box, which stays open after its run ends.
///
/// # Errors
/// [`NameError`] when `due_at` is outside the years the clock can print, or the name is too long.
pub fn box_name(
    id: WorkflowId,
    workflow: &str,
    due_at: Ts,
    cause: &RunCause,
) -> Result<BoxName, NameError> {
    let invalid = || NameError::BoxName(format!("{workflow}@{}", due_at.as_millis()));
    let stamp = |format: &str| -> Result<String, NameError> {
        Ok(jiff::Timestamp::from_millisecond(due_at.as_millis())
            .map_err(|_| invalid())?
            .strftime(format)
            .to_string())
    };
    let label = match cause {
        RunCause::Cron => stamp("%Y%m%d%H%M")?,
        RunCause::Manual { pull: None } => format!("m{}", stamp("%Y%m%d%H%M%S")?),
        RunCause::Manual { pull: Some(pull) } => format!(
            "pr{}-{}-m{}",
            pull.number,
            pull.head_sha.short(),
            stamp("%Y%m%d%H%M%S")?
        ),
        RunCause::PullRequest { pull } => format!("pr{}-{}", pull.number, pull.head_sha.short()),
    };
    BoxName::parse(&format!("{}/w{id}-{label}", slug(workflow, PART_MAX)))
}
