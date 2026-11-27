//! `ask`: a question about some repos, answered by an agent place started for it.
//!
//! The ask starts a place that holds the claim and the question from its insert. Anyone signed in
//! may ask. A place whose rules were not served at launch gets them from the inbox pass, before
//! the question is sent.

use moor_core::config::Flags;
use moor_core::domain::place::{BoxKind, Claim, PlaceState};
use moor_core::request::AskReq;
use moor_core::view::ask::AskValue;
use moor_core::{BoxId, BoxName, ErrorKind};

use super::common::box_exec;
use super::common::facts::settings_of;
use super::start;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{BoxRow, Store};

/// Starts a place for `req` that holds the question.
///
/// # Errors
/// Usage for an ask with no repo or no question; Refused for a caller missing a secret the repos'
/// rules need, or unable to read a repo; otherwise a failed start or store write.
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
    let name = BoxName::parse_allow_bare(&format!("place-{:08x}", cx.rng().u32()))
        .map_err(|e| CommandError::new(ErrorKind::Internal, e))?;
    let id = super::place::start_claimed(cx, store, name, claim)?;
    value(store, id)
}

/// The view of place `id` after an ask.
fn value(store: &Store, id: BoxId) -> Result<AskValue, CommandError> {
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
        claimed_at: claim.at,
        served_at: claim.served,
        steered_at: claim.steered,
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
