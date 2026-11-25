//! The Box tools of an agent place: `exec_in_box` runs a command in one of its task's work Boxes,
//! `exec_wait` waits on one still running, `start_box` starts a Box for that user on its task's repos, tied to the task and running no
//! agent, and `close_box` closes one of its task's work Boxes.
//!
//! The gate's Box listener sends them as `box-tool`, naming the place from the Proxy's
//! `X-Moor-Box` and asserting no identity. They answer only a place a task has claimed, act as
//! that place's user and no one else, and reach only the work Boxes the place itself started (a
//! Box's `task` is the place). A Box of another task, of no task or of another user is refused
//! alike. Anything else is
//! refused before a command runs, a Box starts or a Box closes.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::place::{BoxKind, Claim, PlaceState};
use moor_core::request::{BoxCall, BoxRunId, BoxToolReq};
use moor_core::response::{CloseRefusal, ErrorDetail};
use moor_core::user::{Caller, Identity, UserId};
use moor_core::{BoxId, BoxName, BoxTarget, CloseReq, ErrorKind, ExecReq, Response, StartReq};

use super::authz::NOT_YOURS;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::Store;

pub(crate) mod runs;

/// The refusal for a `box-tool` from anyone but a front whose config sets `box_tools: true`.
pub(crate) const FRONT_ONLY: &str =
    "box-tool serves only a front whose controller config sets `box_tools: true`";

/// The refusal for a `box-tool` that asserts an identity: the place's user is the only one.
const NO_IDENTITY: &str = "box-tool acts for the place the Proxy named, so it asserts no identity";

/// The refusal for a call from a work Box's agent.
const FROM_WORK_BOX: &str =
    "exec_in_box, exec_wait, start_box and close_box serve only an agent place \
     that a task has claimed, and this is a work Box. Run the command here instead.";

/// The refusal for a call from a place no task has claimed yet.
const FROM_IDLE_PLACE: &str =
    "exec_in_box, exec_wait, start_box and close_box serve an agent place once a \
     task claims it, and none has yet";

/// The refusal for a Box the place may not reach: another task's, no task's, another user's or
/// none, said alike so that names cannot be probed.
fn not_this_task(target: &BoxName) -> String {
    format!(
        "{NOT_YOURS}: Box {} is not a work Box this task started; it belongs to another task \
         or none",
        target.as_str()
    )
}

/// The refusal for a target that is an agent place.
const ONLY_WORK_BOXES: &str = "exec_in_box and close_box reach only work Boxes, not an agent place";

/// What a place's call runs, once it is allowed.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Plan {
    /// A command in a work Box of the place's task, named by id.
    Exec(ExecReq),
    /// A start for the place's user on the claim's repos, tied to the place and running no agent.
    Start(Box<StartReq>),
    /// The close of a work Box of the place's task, named by id. It carries no `force`.
    Close(CloseReq),
    /// A wait on a command the place's user started, which [`runs::answer`] scopes to the place.
    Wait(BoxRunId),
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
/// that is not a claimed place, a target that is not an open work Box the place started, or a
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
            target: BoxTarget::Id(work_box(store, (user, req.from), target)?),
            argv: vec![command.clone()],
            timeout_secs: Some(timeout.get()),
        }),
        BoxCall::Start {
            name,
            repo,
            also,
            no_setup,
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
                repo: Some(repo.clone()),
                no_agent: true,
                no_setup: *no_setup,
                task: Some(req.from),
                // Named even when empty, so the user config's `box.also` adds no repo.
                also: Some(also.clone()),
                ..StartReq::named(name.clone())
            }))
        }
        BoxCall::Close { target } => Plan::Close(CloseReq {
            target: BoxTarget::Id(closable_box(store, (user, req.from), target)?),
            force: false,
        }),
        BoxCall::Wait { run } => Plan::Wait(*run),
    };
    Ok(Admitted { user, who, plan })
}

/// A close refusal as the place's agent can act on it: it has no CLI and no force, so it is told
/// to push the work with `exec_in_box` or to tell its user. Any other reply is as it was.
pub(crate) fn for_place(mut reply: Response) -> Response {
    let Some(error) = reply.error.as_mut() else {
        return reply;
    };
    let said = match &error.detail {
        Some(ErrorDetail::CloseRefused(CloseRefusal::Unsaved {
            branch,
            ahead,
            dirty,
        })) => format!(
            "the Box holds work that is not pushed ({ahead} unpushed commit(s), {dirty} \
             uncommitted file(s)). To keep it, commit it and push it with exec_in_box, such as \
             `git push origin HEAD:refs/heads/{branch}`, then call close_box again; or tell \
             your user the Box holds work that is not pushed."
        ),
        Some(ErrorDetail::CloseRefused(CloseRefusal::Unchecked { reason })) => format!(
            "could not check the Box for unsaved work: {reason}. Call close_box again once \
             the Box can be reached, or tell your user."
        ),
        _ => return reply,
    };
    error.message = said;
    error.hints.clear();
    reply
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

/// The id of `target`, an open work Box of `user` that `place` started.
fn work_box(
    store: &Store,
    (user, place): (UserId, BoxId),
    target: &BoxName,
) -> Result<BoxId, CommandError> {
    let row = store
        .open_box_by_name(target)?
        .filter(|row| row.user == Some(user))
        .ok_or_else(|| refused(not_this_task(target)))?;
    match row.kind {
        BoxKind::Work if row.task == Some(place) => Ok(row.id),
        BoxKind::Work => Err(refused(not_this_task(target))),
        BoxKind::Place(_) => Err(refused(ONLY_WORK_BOXES)),
    }
}

/// The id of `target`, an open work Box of `user` that `place` started, which a close may reach. One that closed
/// already is named so, since the end state the caller asked for is the one it finds.
fn closable_box(
    store: &Store,
    who: (UserId, BoxId),
    target: &BoxName,
) -> Result<BoxId, CommandError> {
    let open = work_box(store, who, target);
    if open.is_err() && closed_work_box(store, who, target)? {
        return Err(refused(format!(
            "Box {} is already closed",
            target.as_str()
        )));
    }
    open
}

/// Whether the newest Box named `target` is a closed work Box of `user` that `place` started.
fn closed_work_box(
    store: &Store,
    (user, place): (UserId, BoxId),
    target: &BoxName,
) -> Result<bool, CommandError> {
    Ok(store.latest_box_by_name(target)?.is_some_and(|row| {
        row.user == Some(user)
            && row.task == Some(place)
            && matches!(row.kind, BoxKind::Work)
            && row.lifecycle == BoxLifecycle::Closed
    }))
}

#[cfg(test)]
mod tests;
