//! `ask`: a question about some repos, answered by the caller's idle place.
//!
//! The ask claims the caller's oldest ready idle place and queues the question for its agent in
//! one write, then delivers it. The delivery gives the place the repos' rules first and sends the
//! question only once they are served, so a delivery cut off part way finishes on the inbox pass.
//! With no idle place it starts a place that holds the claim and the question from its insert.

use moor_core::config::Flags;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::hands::HandsState;
use moor_core::domain::place::{BoxKind, Claim, PlaceState};
use moor_core::request::AskReq;
use moor_core::view::ask::AskValue;
use moor_core::{BoxId, BoxName, ErrorKind, StartReq};

use super::common::box_exec;
use super::common::facts::settings_of;
use super::start;
use crate::cx::owners::Cmd;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{BoxRow, Store};

/// Claims the caller's idle place for `req` and sends it the question, or starts a place for it.
///
/// # Errors
/// Usage for an ask with no repo or no question; Refused for a caller off the `agent_places`
/// list, missing a secret the repos' rules need, or unable to read a repo; otherwise a failed
/// start, store write or delivery.
pub(crate) fn run(cx: &Cx, req: &AskReq) -> Result<AskValue, CommandError> {
    let Some((repo, also)) = req.repos.split_first() else {
        return Err(CommandError::new(ErrorKind::Usage, "ask names no repo")
            .with_hint("name one with --repo OWNER/NAME"));
    };
    if req.prompt.trim().is_empty() {
        return Err(CommandError::new(ErrorKind::Usage, "ask has no question"));
    }
    let store = cx.store()?;
    let user = start::require(cx, Some(repo))?;
    super::place::on_roster(cx, store, user)?;
    for r in &req.repos {
        start::repo::fetch(cx, user, r, None)?;
    }
    let claim = Claim {
        repo: repo.clone(),
        also: also.to_vec(),
        prompt: req.prompt.clone(),
        at: cx.clock().now().as_millis(),
        served: None,
        steered: None,
    };
    let text = claim.first_message();
    for row in store.idle_places_of(user)?.iter().filter(|r| ready(r)) {
        let Some(turn) = cx.try_turn(row.id, Cmd::Steer) else {
            continue;
        };
        if !store.claim_place(row.id, user, &claim, &text)? {
            continue;
        }
        super::steer::deliver(cx, store, row.id)?;
        drop(turn);
        return value(store, row.id, true);
    }
    let why = super::place::why_none(store, user)?;
    let mut v = cold(cx, store, claim, &text)?;
    v.no_idle_place = Some(why);
    Ok(v)
}

/// Whether idle place `row` can take a question now, by what the store holds: awake on a ready
/// machine with an agent that launched.
fn ready(row: &BoxRow) -> bool {
    row.lifecycle == BoxLifecycle::Ready
        && !row.asleep()
        && row.launch_failure.is_none()
        && row
            .hands
            .as_ref()
            .is_some_and(|h| h.state == HandsState::Ready)
}

/// Starts a place that holds `claim` from its insert, with `text` as its agent's first prompt.
fn cold(cx: &Cx, store: &Store, claim: Claim, text: &str) -> Result<AskValue, CommandError> {
    let name = BoxName::parse_allow_bare(&format!("place-{:08x}", cx.rng().u32()))
        .map_err(|e| CommandError::new(ErrorKind::Internal, e))?;
    let req = StartReq {
        name,
        prompt: Some(text.to_owned()),
        issue: None,
        base: None,
        model: None,
        node: None,
        placement: moor_core::request::Placement::default(),
        place: false,
        no_agent: false,
        no_setup: false,
        cold: false,
        resume: false,
        repo: None,
        also: None,
        config: None,
        parent: None,
        gates: None,
        identity: None,
        task: None,
    };
    let v = start::run_as(cx, &req, BoxKind::Place(PlaceState::Claimed(claim)))?;
    let id = BoxId::new(v.boxed.id);
    if v.agent.launched {
        // The launch planned the claim's rules before the agent ran, and carried the question.
        let now = cx.clock().now().as_millis();
        store.claim_served(id, now)?;
        store.claim_steered(id, now)?;
    }
    value(store, id, false)
}

/// The view of place `id` after an ask.
fn value(store: &Store, id: BoxId, claimed: bool) -> Result<AskValue, CommandError> {
    let row = store
        .box_row(id)?
        .ok_or_else(|| CommandError::new(ErrorKind::Internal, "the asked place is gone"))?;
    let BoxKind::Place(PlaceState::Claimed(claim)) = &row.kind else {
        return Err(CommandError::new(
            ErrorKind::Internal,
            "the asked place holds no claim",
        ));
    };
    Ok(AskValue {
        box_name: row.name.as_str().to_owned(),
        claimed,
        claimed_at: claim.at,
        served_at: claim.served,
        steered_at: claim.steered,
        no_idle_place: None,
    })
}

/// Gives claimed place `row` its claim's rules when they are not served yet, before any row of
/// its inbox is sent: the Proxy spec is stored and the place's egress takes the repos' hosts.
/// A failure leaves the claim unserved, and its question queued for the next delivery.
///
/// # Errors
/// Failed when the rules cannot be served; a store failure.
pub(super) fn serve(cx: &Cx, store: &Store, row: &BoxRow) -> Result<(), CommandError> {
    let BoxKind::Place(PlaceState::Claimed(Claim { served: None, .. })) = &row.kind else {
        return Ok(());
    };
    let hands = row
        .hands
        .as_ref()
        .and_then(|h| Some((h.handle.clone()?, h.inc)));
    let cfg = settings_of(cx, row, &Flags::default())?;
    box_exec::proxy(cx, row.id, hands, None, &cfg).map_err(|why| {
        CommandError::new(
            ErrorKind::Failed,
            format!(
                "could not give place {} its rules: {why}",
                row.name.as_str()
            ),
        )
    })?;
    store.claim_served(row.id, cx.clock().now().as_millis())?;
    Ok(())
}

/// Records that claimed place `row`'s question reached its agent, once.
///
/// # Errors
/// A store failure.
pub(super) fn steered(cx: &Cx, store: &Store, row: &BoxRow) -> Result<(), CommandError> {
    if matches!(&row.kind, BoxKind::Place(PlaceState::Claimed(c)) if c.steered.is_none()) {
        store.claim_steered(row.id, cx.clock().now().as_millis())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
