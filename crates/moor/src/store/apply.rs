//! The only writer of box/box state and events: `Store::apply` plus run records.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::cache_save::CacheSave;
use moor_core::domain::decision::{BoxTarget, Decision, Field, HandsFields, IntentChange};
use moor_core::domain::event::{box_events, lifecycle_events, session_events, Event};
use moor_core::domain::hands::{HandsInc, HandsInput, HandsState};
use moor_core::domain::session_state::SessionState;
use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use moor_schema::tables::{BoxHands, BoxState, Events, Sessions};
use sea_query::{Expr, ExprTrait as _, Query, Value};

mod sql;
mod steps;

pub(super) use self::sql::update;
use self::sql::{box_field_sets, branch_taken, driver_field_sets, session_field_sets, Sets};
use self::steps::{step_box, Parts};
use super::box_node::live_on_node_in;
use super::hands::{fence, first_hands, update_hands};
use super::select::{newest_session, state_update};
use super::{HandsRow, Store, StoreError};
use crate::store::db::{self, DbExt as _, Tx};
use moor_core::name::NodeName;

/// Result of a committed `apply`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Applied {
    /// The box written.
    pub(crate) box_id: BoxId,
    /// Box lifecycle after the apply.
    pub(crate) lifecycle: BoxLifecycle,
    /// Primary Session state after the apply.
    pub(crate) session: SessionState,
    /// Box state after the apply, if the box has a box.
    pub(crate) box_state: Option<HandsState>,
    /// Ids of the events inserted, in order.
    pub(crate) events: Vec<i64>,
}

/// A state step: before (if any) and after.
struct Step<S> {
    /// State before; `None` for a fresh row.
    from: Option<S>,
    /// State after.
    to: S,
}

impl Store {
    /// Apply a decision atomically: rows and events together, or neither.
    pub(crate) fn apply(&self, d: Decision) -> Result<Applied, StoreError> {
        if let BoxTarget::Existing(id) = d.target {
            self.check_write(id);
        }
        let now = self.now();
        let mut conn = self.writer();
        let tx = conn.immediate()?;
        let applied = apply_in(&tx, now, d)?;
        tx.commit()?;
        drop(conn);
        Ok(applied)
    }

    /// Count the Boxes holding a slot on `node` and the MiB its Boxes hold, let `decide` pick a
    /// decision for Box `id` from those, apply it and record `node` as the Box's.
    ///
    /// The read and the write share one transaction, so no other writer can claim a slot between them.
    /// `decide` returns `None` to write nothing. It runs under the writer lock, so it must not block.
    pub(crate) fn apply_admitting(
        &self,
        id: BoxId,
        node: &NodeName,
        decide: impl FnOnce((usize, usize)) -> Option<Decision>,
    ) -> Result<Option<Applied>, StoreError> {
        self.check_write(id);
        let now = self.now();
        let mut conn = self.writer();
        let tx = conn.immediate()?;
        let Some(d) = decide(live_on_node_in(&tx, node)?) else {
            return Ok(None);
        };
        let applied = apply_in(&tx, now, d)?;
        tx.run(state_update(id).value(BoxState::Node, node.as_str()))?;
        tx.commit()?;
        drop(conn);
        Ok(Some(applied))
    }
}

/// The body of `apply`, inside its transaction; a branch an open Box works on is refused.
fn apply_in(tx: &Tx<'_>, now: Ts, d: Decision) -> Result<Applied, StoreError> {
    let branch = match &d.target {
        BoxTarget::New(new) => new.spec.branch.clone(),
        BoxTarget::Existing(_) => None,
    };
    apply_steps(tx, now, d).map_err(|e| branch_taken(e, branch.as_ref()))
}

/// Every write of `d`, inside its transaction.
fn apply_steps(tx: &Tx<'_>, now: Ts, d: Decision) -> Result<Applied, StoreError> {
    let steps::Stepped {
        id,
        existing,
        parts,
        from,
    } = step_box(tx, now, &d)?;
    fence(id, existing.as_ref(), &d)?;
    let quiet = Step {
        from: from.map(is_quiet),
        to: is_quiet(parts),
    };
    let box_step = step_driver(tx, now, id, existing.as_ref(), &d)?;
    write_box_row(tx, id, now, &d, &quiet, parts)?;
    let inc = existing.as_ref().map_or(HandsInc::FIRST, |h| h.inc);
    write_box(tx, id, now, (&d, inc), box_step.as_ref(), &quiet)?;
    let mut ids = Vec::new();
    let stepped = match (&d.lifecycle_input, &d.session_input) {
        (Some(l), _) => lifecycle_events(from.map(|p| p.0), parts.0, l),
        (None, Some(s)) => session_events(s),
        (None, None) => Vec::new(),
    };
    let stepped = with_setup_why(
        with_cache_save(stepped, d.cache_save),
        d.setup_why.as_deref(),
    );
    for e in &stepped {
        ids.push(insert_event(tx, id, e, now)?);
    }
    let mut events = Vec::new();
    if let (Some(i), Some(s)) = (d.input_to_hands(), &box_step) {
        events.extend(box_events(
            s.from,
            s.to,
            i,
            existing
                .as_ref()
                .and_then(|b| b.handle.as_ref().or(b.attempted_handle.as_ref())),
        ));
    }
    events.extend(d.events.into_iter().map(Event::from));
    for e in &events {
        ids.push(insert_event(tx, id, e, now)?);
    }
    Ok(Applied {
        box_id: id,
        lifecycle: parts.0,
        session: parts.1,
        box_state: box_step.map(|s| s.to),
        events: ids,
    })
}

/// Step the box state, recording the provider on the first input.
fn step_driver(
    tx: &Tx<'_>,
    now: Ts,
    id: BoxId,
    existing: Option<&HandsRow>,
    d: &Decision,
) -> Result<Option<Step<HandsState>>, StoreError> {
    let Some(input) = d.input_to_hands() else {
        return Ok(existing.map(|b| Step {
            from: Some(b.state),
            to: b.state,
        }));
    };
    if let Some(b) = existing {
        let to = b.state.step(input)?;
        return Ok(Some(Step {
            from: Some(b.state),
            to,
        }));
    }
    let to = HandsState::initial(input)?;
    let Some(Field::Set(provider)) = d.hands.as_ref().map(|w| &w.fields.provider) else {
        return Err(StoreError::Invalid("new box needs box_provider".into()));
    };
    tx.run(first_hands(id, provider, to, now))?;
    Ok(Some(Step { from: None, to }))
}

/// `events` with an `asleep` event saying what its sleep decided about the caches.
fn with_cache_save(mut events: Vec<Event>, save: Option<CacheSave>) -> Vec<Event> {
    for e in &mut events {
        if let Event::Asleep { cache_save, .. } = e {
            *cache_save = save;
        }
    }
    events
}

/// `events` with a `setup_failed` event saying why its line failed.
fn with_setup_why(mut events: Vec<Event>, why: Option<&str>) -> Vec<Event> {
    for e in &mut events {
        if let Event::SetupFailed { why: slot, .. } = e {
            *slot = why.map(str::to_owned);
        }
    }
    events
}

/// Write box state, fields and intent.
fn write_box_row(
    tx: &Tx<'_>,
    id: BoxId,
    now: Ts,
    d: &Decision,
    step: &Step<bool>,
    (lifecycle, session_state): Parts,
) -> Result<(), StoreError> {
    let mut sets = box_field_sets(&d.set);
    let mut session = session_field_sets(&d.set);
    let stepped = d.lifecycle_input.is_some() || d.session_input.is_some();
    if stepped {
        sets.push((BoxState::State, lifecycle.as_str().into()));
        if !lifecycle.is_live() {
            sets.push((BoxState::OpenName, Value::String(None)));
            sets.push((BoxState::OpenBranch, Value::String(None)));
        }
        sets.extend(idle_since_sets(step, now));
        session.push((Sessions::State, session_state.as_str().into()));
        if session_state == SessionState::Ended {
            session.push((Sessions::EndedAt, now.as_millis().into()));
        }
    }
    sets.extend(intent_sets(d.intent, now));
    sets.push((BoxState::UpdatedAt, now.as_millis().into()));
    update(tx, &mut state_update(id), &sets)?;
    let newest = Query::select()
        .expr(Expr::col(Sessions::Ordinal).max())
        .from(Sessions::Table)
        .and_where(Expr::col(Sessions::Box).eq(id.get()))
        .take();
    update(
        tx,
        Query::update()
            .table(Sessions::Table)
            .and_where(Expr::col(Sessions::Box).eq(id.get()))
            .and_where(Expr::col(Sessions::Ordinal).eq(db::sub(newest))),
        &session,
    )?;
    if stepped && !lifecycle.is_live() {
        tx.run(
            Query::update()
                .table(Sessions::Table)
                .value(Sessions::State, "ended")
                .value(Sessions::EndedAt, now.as_millis())
                .and_where(Expr::col(Sessions::Box).eq(id.get()))
                .and_where(Expr::col(Sessions::State).ne("ended")),
        )?;
        super::users::machine::release_in(tx, id, now)?;
    }
    Ok(())
}

/// A Ready Box whose primary Session is idle or ended.
fn is_quiet((lifecycle, session): Parts) -> bool {
    lifecycle == BoxLifecycle::Ready && session.is_quiet()
}

/// `idle_since` follows quietness: stamped on becoming quiet, cleared on leaving it. Becoming
/// quiet ends a turn, so the Box's archive falls due.
fn idle_since_sets(step: &Step<bool>, now: Ts) -> Sets<BoxState> {
    match (step.from, step.to) {
        (from, true) if from != Some(true) => vec![
            (BoxState::IdleSince, now.as_millis().into()),
            (BoxState::ArchiveDue, now.as_millis().into()),
        ],
        (Some(true), false) => vec![(BoxState::IdleSince, Value::BigInt(None))],
        _ => Vec::new(),
    }
}

/// Write box state, fields and handles; box fields without a box are invalid. A new handle is
/// a fresh disk, so the egress token delivered to the old one is delivered again.
///
/// A step out of quiet drops the recorded sleep refusal, so the next Idle stretch asks again.
/// The writes go to the current hands `inc`, which [`fence`] checked `d` against.
fn write_box(
    tx: &Tx<'_>,
    id: BoxId,
    now: Ts,
    (d, inc): (&Decision, HandsInc),
    step: Option<&Step<HandsState>>,
    quiet: &Step<bool>,
) -> Result<(), StoreError> {
    let none = HandsFields::default();
    let mut sets = driver_field_sets(d.hands.as_ref().map_or(&none, |w| &w.fields));
    if quiet.from == Some(true) && !quiet.to {
        if let Some(i) = sets.iter().position(|(c, _)| *c == BoxHands::SleepReason) {
            sets.remove(i);
        }
        sets.push((BoxHands::SleepReason, Value::String(None)));
    }
    let Some(step) = step else {
        if sets.is_empty() {
            return Ok(());
        }
        return Err(StoreError::Invalid(
            "Box fields set but the Box has no machine".into(),
        ));
    };
    if let Some(input) = d.input_to_hands() {
        sets.push((BoxHands::State, step.to.as_str().into()));
        // A guest that may run again can change, so what was checked before no longer stands.
        if matches!(input, HandsInput::Wake | HandsInput::Acquired { .. }) {
            for column in [
                BoxHands::SuspendAhead,
                BoxHands::SuspendDirty,
                BoxHands::SuspendCheckedAt,
                BoxHands::SuspendPublish,
            ] {
                sets.push((column, None::<i64>.into()));
            }
        }
        sets.extend(input_handle_sets(input, &sets));
    }
    if d.input_to_hands().is_some() {
        sets.push((BoxHands::StateAt, now.as_millis().into()));
    }
    if sets.iter().any(|(c, _)| *c == BoxHands::Handle) {
        super::proxy::forget_egress_delivery(tx, id)?;
    }
    update(tx, &mut update_hands(id, inc), &sets)
}

/// Handles carried by a box input, unless the Fields already set that column.
fn input_handle_sets(input: &HandsInput, taken: &Sets<BoxHands>) -> Sets<BoxHands> {
    let (col, handle) = match input {
        HandsInput::Acquired { handle } => (BoxHands::Handle, Some(handle)),
        HandsInput::AcquireFailed { attempted, .. } | HandsInput::AcquireCut { attempted } => {
            (BoxHands::AttemptedHandle, attempted.as_ref())
        }
        _ => return Vec::new(),
    };
    match handle {
        Some(h) if !taken.iter().any(|(c, _)| *c == col) => {
            vec![(col, h.as_str().into())]
        }
        _ => Vec::new(),
    }
}

/// Column assignments for an intent change.
pub(super) fn intent_sets(change: IntentChange, now: Ts) -> Sets<BoxState> {
    let (kind, run, at) = match change {
        IntentChange::Keep => return Vec::new(),
        IntentChange::Clear => (None, None, None),
        IntentChange::Set(i) => (
            Some(i.kind().to_owned()),
            i.run().map(moor_core::RunId::get),
            Some(now.as_millis()),
        ),
    };
    // Written, never read: the previous binary, which a failed deploy rolls back to, refuses an
    // intent without a pid until a later migration drops the column.
    let pid = kind.as_ref().map(|_| i64::from(std::process::id()));
    vec![
        (BoxState::IntentKind, kind.into()),
        (BoxState::IntentPid, pid.into()),
        (BoxState::IntentRun, run.into()),
        (BoxState::IntentAt, at.into()),
    ]
}

/// Insert one event row, stamped with the Box's newest Session, and return its id.
pub(super) fn insert_event(tx: &Tx<'_>, id: BoxId, e: &Event, now: Ts) -> Result<i64, StoreError> {
    let event: i64 = tx.row(
        Query::insert()
            .into_table(Events::Table)
            .columns([
                Events::Box,
                Events::Session,
                Events::Kind,
                Events::Payload,
                Events::Ts,
            ])
            .values_panic([
                id.get().into(),
                db::sub(newest_session(id)),
                e.kind().into(),
                e.payload().to_string().into(),
                now.as_millis().into(),
            ])
            .returning_col(Events::Id),
        |r| r.get(0),
    )?;
    super::webhook_outbox::enqueue(tx, id, event, e.kind())?;
    Ok(event)
}

#[cfg(test)]
mod tests;
