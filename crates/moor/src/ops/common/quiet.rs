//! What the passes read of a Box to tell whether its agent is quiet.

use moor_core::decide::workflow::{quiet_since, RunBoxFacts};
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::session_state::SessionState;
use moor_core::seam::clock::Ts;
use moor_core::BoxId;

use crate::cx::Cx;
use crate::store::{BoxRow, Store, StoreError};

/// What a pass reads of Box `row` to tell where its agent stands.
///
/// A sleep clears the Box's `idle_since`, so an asleep Box whose agent went quiet before the
/// sleep reads as idle since its last `idled` event. One put to sleep while its agent worked
/// has no such time: the cut turn did not finish its step.
pub(crate) fn box_facts(store: &Store, row: &BoxRow) -> Result<RunBoxFacts, StoreError> {
    let idle_since = if row.lifecycle == BoxLifecycle::Asleep {
        match row.parked_at_work {
            Some(_) => None,
            None => store.newest_event_of(row.id, "idled")?.map(|e| e.ts),
        }
    } else {
        row.idle_since
    };
    Ok(RunBoxFacts {
        lifecycle: row.lifecycle,
        session: row.session,
        idle_since,
        launch_failed: row.launch_failure.is_some(),
        death_capped: row.lifecycle == BoxLifecycle::Asleep && death_capped(store, row.id)?,
    })
}

/// Whether Box `id` died and moor did not resume it, and no wake has come since. Events
/// are numbered in the order they were written, so ids tell which came last.
fn death_capped(store: &Store, id: BoxId) -> Result<bool, StoreError> {
    let Some(capped) = store.newest_event_of(id, "death_capped")? else {
        return Ok(false);
    };
    let woke = store.newest_event_of(id, "woke")?;
    Ok(woke.is_none_or(|w| w.id < capped.id))
}

/// Whether the agent of Box `row` is quiet enough for moor to act on it: idle since the last steer
/// or ended, with nothing waiting in its inbox.
pub(crate) fn settled(cx: &Cx, store: &Store, row: &BoxRow) -> Result<bool, StoreError> {
    let steered = store.last_event_at(row.id, &["steered"])?;
    let facts = box_facts(store, row)?;
    Ok(quiet_for_check(&facts, steered, cx.clock().now()) && store.unsent(row.id)?.is_empty())
}

/// Whether moor may tell the agent of Box `row`, or its owner, that a gate waits: the agent is
/// [`settled`], or the Box sleeps with nothing in its inbox. A sleep ends the agent's turn, even
/// one it cut, and a message waits in the inbox until it wakes the Box. A cut turn is not judged,
/// though: checks still wait for [`settled`]. One whose places died is left alone, as only a
/// person resumes it.
pub(crate) fn may_tell(cx: &Cx, store: &Store, row: &BoxRow) -> Result<bool, StoreError> {
    if row.lifecycle == BoxLifecycle::Asleep
        && store.unsent(row.id)?.is_empty()
        && !death_capped(store, row.id)?
    {
        return Ok(true);
    }
    settled(cx, store, row)
}

/// Whether moor may tell the agent of Box `row` news it waits for, such as a gate that passed:
/// nothing waits in its inbox, and the Box is awake or [`may_tell`] allows it. News needs no quiet
/// agent. The inbox holds it until the agent can take it, and an agent that ended its turn beside
/// background work it left running reads as working, though it may be waiting for that news.
pub(crate) fn may_tell_news(cx: &Cx, store: &Store, row: &BoxRow) -> Result<bool, StoreError> {
    if row.lifecycle == BoxLifecycle::Asleep {
        return may_tell(cx, store, row);
    }
    Ok(store.unsent(row.id)?.is_empty())
}

/// Whether the agent of a Box with `facts` is quiet enough for a check to run: idle since the
/// last steer, or ended, in a Box that is up or asleep.
pub(crate) fn quiet_for_check(facts: &RunBoxFacts, steered: Option<Ts>, now: Ts) -> bool {
    if !matches!(facts.lifecycle, BoxLifecycle::Ready | BoxLifecycle::Asleep) {
        return false;
    }
    match facts.session {
        SessionState::Ended => true,
        SessionState::Idle => steered.is_none_or(|t| quiet_since(facts, t, now)),
        SessionState::Starting | SessionState::Working => false,
    }
}
