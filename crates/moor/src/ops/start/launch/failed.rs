//! Records how an agent launch failed on its Box.

use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields};
use moor_core::seam::engine::EngineError;
use moor_core::view::{AgentLaunchView, LaunchFailure};
use moor_core::BoxId;

use super::TAIL_LINES;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::view::rfc3339;
use crate::store::Store;

/// Records a failed launch on the Box, which stays Working with no session, and reports it.
pub(super) fn record_failure(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    e: &EngineError,
) -> Result<AgentLaunchView, CommandError> {
    let failure = failure(cx, e);
    record(store, id, Field::Set(to_json(&failure)?), Some(&failure))?;
    Ok(AgentLaunchView {
        launched: false,
        session: None,
        reason: Some(failure),
        nonce: None,
    })
}

/// The failure to record for `e`: the first line is the message, the last lines after it the tail.
fn failure(cx: &Cx, e: &EngineError) -> LaunchFailure {
    let (step, text) = match e {
        EngineError::Failed { step, message } => ((*step).to_owned(), message.clone()),
        other => ("launch".to_owned(), other.to_string()),
    };
    let (message, rest) = text.split_once('\n').unwrap_or((&text, ""));
    let lines: Vec<&str> = rest.lines().collect();
    let keep = if step == "proxy" {
        crate::ops::common::proxy::TAIL_LINES
    } else {
        TAIL_LINES
    };
    let tail = lines
        .iter()
        .skip(lines.len().saturating_sub(keep))
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    LaunchFailure {
        step,
        message: message.to_owned(),
        tail: Some(tail).filter(|t| !t.is_empty()),
        at: rfc3339(cx.clock().now()),
    }
}

/// The failure as the JSON kept on the Box row.
pub(in crate::ops::start) fn to_json(f: &LaunchFailure) -> Result<String, CommandError> {
    serde_json::to_string(f).map_err(|e| CommandError::new(moor_core::ErrorKind::Internal, e))
}

/// Writes the Box's launch failure column, with a `session_failed` event for a new failure.
pub(in crate::ops::start) fn record(
    store: &Store,
    id: BoxId,
    column: Field<String>,
    failed: Option<&LaunchFailure>,
) -> Result<(), CommandError> {
    let set = Fields {
        launch_failure: column,
        ..Fields::default()
    };
    let mut decision = Decision::new(id).set(set);
    if let Some(f) = failed {
        decision = decision.event(ExtraEvent::SessionFailed {
            step: f.step.clone(),
            message: f.message.clone(),
        });
    }
    store.apply(decision)?;
    Ok(())
}
