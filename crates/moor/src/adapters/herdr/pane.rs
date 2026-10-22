//! Pane observation: herdr's status for a pane plus the end of its text.

use moor_core::activity::{PaneObs, PaneStatus};
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::{PaneRef, SurfaceError};
use serde::Deserialize;

use super::errors::HerdrFailure;
use super::Herdr;

/// Text lines read from the end of a pane.
const TAIL_LINES: &str = "60";
/// Start of the line moor prints when the agent ends.
const EXIT_MARKER: &str = "moor: agent exited ";

/// `pane list` result.
#[derive(Deserialize)]
struct Listing {
    /// Panes in the session.
    #[serde(default)]
    panes: Vec<Entry>,
}

/// One listed pane.
#[derive(Deserialize)]
struct Entry {
    /// Pane id.
    pane_id: String,
    /// herdr's agent status word.
    agent_status: Option<String>,
}

/// Wrapper around the listing.
#[derive(Deserialize)]
struct Envelope {
    /// Payload.
    result: Listing,
}

/// Maps herdr's status word; anything else is `Unknown`.
fn status_of(word: Option<&str>) -> PaneStatus {
    match word {
        Some("working") => PaneStatus::Working,
        Some("idle") => PaneStatus::Idle,
        Some("blocked") => PaneStatus::Blocked,
        _ => PaneStatus::Unknown,
    }
}

/// The status of `pane` in `out`, `None` when the pane is not listed.
fn listed_status(out: &str, pane: &PaneRef) -> Result<Option<PaneStatus>, SurfaceError> {
    let env: Envelope = serde_json::from_str(out).map_err(|e| SurfaceError::Failed {
        summary: format!("unreadable pane list output: {e}"),
    })?;
    Ok(env
        .result
        .panes
        .iter()
        .find(|p| p.pane_id == pane.as_str())
        .map(|p| status_of(p.agent_status.as_deref())))
}

/// The code on the last exit marker line in `text`. The marker must start
/// the line and be followed at once by digits; leftover screen text after
/// the digits is ignored, while a marker mid-line (such as an echoed command)
/// is not one.
fn exit_marker(text: &str) -> Option<i32> {
    text.lines().rev().find_map(|line| {
        let rest = line.trim_start().strip_prefix(EXIT_MARKER)?;
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        rest.get(..digits)?.parse::<i32>().ok()
    })
}

impl Herdr {
    /// Runs one herdr call within `deadline`; a down server is not started.
    fn call_within(&self, args: &[&str], deadline: Deadline) -> Result<String, SurfaceError> {
        match self.run_within(args, deadline) {
            Ok(out) => Ok(out),
            Err(HerdrFailure::Surface(e)) => Err(e),
            Err(HerdrFailure::ServerNotRunning) => Err(SurfaceError::Unavailable(
                "herdr server not running".to_owned(),
            )),
        }
    }

    /// Observes `pane`; each herdr call gets `deadline`.
    pub(super) fn observe(
        &self,
        pane: &PaneRef,
        deadline: Deadline,
    ) -> Result<PaneObs, SurfaceError> {
        let listing = self.call_within(&["pane", "list"], deadline)?;
        let status = listed_status(&listing, pane)?.ok_or(SurfaceError::NotFound)?;
        let text = self.call_within(
            &["pane", "read", pane.as_str(), "--lines", TAIL_LINES],
            deadline,
        )?;
        Ok(PaneObs {
            status,
            prompt: None,
            exit_marker: exit_marker(&text),
            tail: text.trim_end().to_owned(),
        })
    }
}

#[cfg(test)]
mod tests;
