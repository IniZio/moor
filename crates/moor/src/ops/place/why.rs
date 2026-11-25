//! Why a user's ask found no idle place: the answer names what stands in the keeper's way.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::user::UserId;

use super::{failure, rejected, Standing};
use crate::error::CommandError;
use crate::store::{BoxRow, Store};

/// Why `user`, whose ask found no idle place to claim, had none: the first of their unclaimed
/// places and how it stands, else their newest place's rejection, else none at all. Each failed
/// or stuck place is named, so its person can close it and let the keeper start the next.
///
/// # Errors
/// A store failure.
pub(crate) fn why_none(store: &Store, user: UserId) -> Result<String, CommandError> {
    let why = if let Some(row) = store.idle_places_of(user)?.first() {
        let name = row.name.as_str();
        match Standing::of(row) {
            Standing::Failed => format!(
                "{name} failed: {}; close it with close_box {name} and moor starts the next place",
                failure(row)
            ),
            Standing::Lost => {
                format!("{name} lost its machine; moor closes it and starts the next place")
            }
            Standing::Waiting if row.lifecycle == BoxLifecycle::Ready => {
                format!("{name} is ready but another ask holds it")
            }
            Standing::Waiting => format!(
                "{name} is still coming up since {}; close it with close_box {name} if it is stuck",
                crate::ops::common::view::rfc3339(row.created_at)
            ),
        }
    } else {
        match store.newest_place_of(user)? {
            Some(row) if rejected(Some(&row)) => format!(
                "{} was rejected at start{}",
                row.name.as_str(),
                rejection(store, &row)?.map_or_else(String::new, |why| format!(": {why}")),
            ),
            _ => "none was waiting".to_owned(),
        }
    };
    Ok(format!("no idle place: {why}"))
}

/// Why the rejected place `row` was rejected, as its `rejected` event says.
pub(super) fn rejection(store: &Store, row: &BoxRow) -> Result<Option<String>, CommandError> {
    if row.state_reason.is_some() {
        return Ok(row.state_reason.clone());
    }
    let events = store.events(row.id, 20)?;
    Ok(events
        .iter()
        .rev()
        .find(|e| e.kind == "rejected")
        .and_then(|e| e.payload.get("reason")?.as_str().map(str::to_owned)))
}
