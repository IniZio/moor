//! The Box tools of an agent place: `exec_in_box` runs a command in one of its user's work Boxes,
//! and `start_box` starts a Box for that user on its task's repos.
//!
//! The gate's Box listener sends them as `box-tool`, naming the place from the Proxy's
//! `X-Moor-Box` and asserting no identity. They answer only a place a task has claimed, act as
//! that place's user and no one else, and reach only that user's work Boxes. Anything else is
//! refused before a command runs or a Box starts.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::place::{BoxKind, Claim, PlaceState};
use moor_core::request::{BoxCall, BoxToolReq};
use moor_core::user::{Caller, Identity, UserId};
use moor_core::{BoxId, BoxName, BoxTarget, ErrorKind, ExecReq, StartReq};

use super::authz::NOT_YOURS;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::Store;

/// The refusal for a `box-tool` from anyone but a front whose config sets `box_tools: true`.
pub(crate) const FRONT_ONLY: &str =
    "box-tool serves only a front whose controller config sets `box_tools: true`";

/// The refusal for a `box-tool` that asserts an identity: the place's user is the only one.
const NO_IDENTITY: &str = "box-tool acts for the place the Proxy named, so it asserts no identity";

/// The refusal for a call from a work Box's agent.
const FROM_WORK_BOX: &str = "exec_in_box and start_box serve only an agent place that a task \
     has claimed, and this is a work Box. Run the command here instead.";

/// The refusal for a call from a place no task has claimed yet.
const FROM_IDLE_PLACE: &str =
    "exec_in_box and start_box serve an agent place once a task claims it, and none has yet";

/// The refusal for a target that is an agent place.
const ONLY_WORK_BOXES: &str = "exec_in_box reaches only work Boxes, not an agent place";

/// What a place's call runs, once it is allowed.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Plan {
    /// A command in a work Box of the place's user, named by id.
    Exec(ExecReq),
    /// A start for the place's user on the claim's repos.
    Start(Box<StartReq>),
}

/// A call the place may make, and whom it acts as.
#[derive(Debug)]
pub(crate) struct Admitted {
    /// The place's user.
    pub(crate) user: UserId,
    /// One of that user's identities, which a start acts as.
    pub(crate) who: Identity,
    /// What runs.
    pub(crate) plan: Plan,
}

/// Refused with `why`.
fn refused(why: impl Into<String>) -> CommandError {
    CommandError::new(ErrorKind::Refused, why.into())
}

/// Allows `req` from `caller`, asserting `asserted`, and says what it runs as whom.
///
/// # Errors
/// The refusal for a caller that is not a `box_tools` front or that asserts an identity, a Box
/// that is not a claimed place, a target that is not the place's user's open work Box, or a
/// start on a repo the claim does not name; or a store failure.
pub(crate) fn admit(
    cx: &Cx,
    caller: &Result<Caller, String>,
    asserted: Option<&Identity>,
    req: &BoxToolReq,
) -> Result<Admitted, CommandError> {
    match caller {
        Ok(Caller::Front(ns)) if cx.env().front_box_tools(ns) => {}
        _ => return Err(refused(FRONT_ONLY)),
    }
    if asserted.is_some() {
        return Err(refused(NO_IDENTITY));
    }
    let store = cx.store()?;
    let (user, claim) = claimed_place(store, req.from)?;
    let who = store
        .identities(user)?
        .into_iter()
        .next()
        .ok_or_else(|| refused("the place's user has no identity"))?;
    let plan = match &req.call {
        BoxCall::Exec {
            target,
            command,
            timeout,
        } => Plan::Exec(ExecReq {
            target: BoxTarget::Id(work_box(store, user, target)?),
            argv: vec![command.clone()],
            timeout_secs: Some(timeout.get()),
        }),
        BoxCall::Start {
            name,
            repo,
            also,
            prompt,
        } => {
            if let Some(other) = std::iter::once(repo)
                .chain(also)
                .find(|r| !claim.repos().any(|c| c == *r))
            {
                let named: Vec<String> = claim.repos().map(ToString::to_string).collect();
                return Err(refused(format!(
                    "start_box may name only this task's repos ({}), not {other}",
                    named.join(", ")
                )));
            }
            Plan::Start(Box::new(StartReq {
                prompt: prompt.clone(),
                repo: Some(repo.clone()),
                // Named even when empty, so the user config's `box.also` adds no repo.
                also: Some(also.clone()),
                ..StartReq::named(name.clone())
            }))
        }
    };
    Ok(Admitted { user, who, plan })
}

/// The user and claim of `from`, an open place a task has claimed.
fn claimed_place(store: &Store, from: BoxId) -> Result<(UserId, Claim), CommandError> {
    let row = store
        .box_row(from)?
        .filter(|row| !matches!(row.lifecycle, BoxLifecycle::Closed | BoxLifecycle::Rejected))
        .ok_or_else(|| refused(format!("Box {} is not open", from.get())))?;
    match row.kind {
        BoxKind::Work => Err(refused(FROM_WORK_BOX)),
        BoxKind::Place(PlaceState::Idle) => Err(refused(FROM_IDLE_PLACE)),
        BoxKind::Place(PlaceState::Claimed(claim)) => row
            .user
            .map(|user| (user, claim))
            .ok_or_else(|| refused("the place acts for no user")),
    }
}

/// The id of `target`, an open work Box of `user`.
fn work_box(store: &Store, user: UserId, target: &BoxName) -> Result<BoxId, CommandError> {
    let row = store
        .open_box_by_name(target)?
        .filter(|row| row.user == Some(user))
        .ok_or_else(|| refused(NOT_YOURS))?;
    match row.kind {
        BoxKind::Work => Ok(row.id),
        BoxKind::Place(_) => Err(refused(ONLY_WORK_BOXES)),
    }
}

#[cfg(test)]
mod tests;
