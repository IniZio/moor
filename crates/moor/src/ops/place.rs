//! Agent places: repo-less Boxes an agent works from, each started cold for one task.
//!
//! A place holds its task's claim from its insert. An `ask` starts one for anyone signed in, and
//! moor starts one to read an automation's prompt.

use moor_core::domain::place::{BoxKind, Claim, PlaceState};
use moor_core::{BoxId, BoxName, StartReq};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{BoxRow, Store};

/// Why the failed place `row` failed.
pub(crate) fn failure(row: &BoxRow) -> String {
    row.state_reason.clone().unwrap_or_else(|| {
        row.launch_failure.as_ref().map_or_else(
            || "its agent did not launch".to_owned(),
            |f| format!("its agent did not launch at {}: {}", f.step, f.message),
        )
    })
}

/// Starts place `name`, which holds `claim` from its insert, with the claim's first message as its
/// agent's first prompt.
///
/// # Errors
/// A failed start or store write.
pub(crate) fn start_claimed(
    cx: &Cx,
    store: &Store,
    name: BoxName,
    claim: Claim,
) -> Result<BoxId, CommandError> {
    let req = StartReq {
        prompt: Some(claim.first_message()),
        placement: moor_core::request::Placement::default(),
        ..StartReq::named(name)
    };
    let v = super::start::run_as(cx, &req, BoxKind::Place(PlaceState::Claimed(claim)))?;
    let id = BoxId::new(v.boxed.id);
    if v.agent.launched {
        // The launch planned the claim's rules before the agent ran, and carried the first message.
        let now = cx.clock().now().as_millis();
        store.claim_served(id, now)?;
        store.claim_steered(id, now)?;
    }
    Ok(id)
}
