//! The only writer of box/box state and events: `Store::apply` plus run records.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::cache_save::CacheSave;
use moor_core::domain::decision::{BoxTarget, Decision, Field, IntentChange};
use moor_core::domain::event::{box_events, lifecycle_events, session_events, Event};
use moor_core::domain::session_state::SessionState;
use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use moor_schema::tables::{BoxRefs, Boxes, Events, Sessions};
use sea_query::{Expr, ExprTrait as _, OnConflict, Query, UpdateStatement, Value};

mod sql;
mod steps;

pub(super) use self::sql::update;
use self::sql::{box_field_sets, branch_taken, driver_field_sets, session_field_sets, Sets};
use self::steps::{step_box, Parts};
use super::box_node::live_on_node_in;
use super::select::newest_session;
use super::{DriverRow, Store, StoreError};
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
    pub(crate) box_state: Option<BoxState>,
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
        let now = self.now();
        let mut conn = self.writer();
        let tx = conn.immediate()?;
        let Some(d) = decide(live_on_node_in(&tx, node)?) else {
            return Ok(None);
        };
        let applied = apply_in(&tx, now, d)?;
        tx.run(box_update(id).value(Boxes::Node, node.as_str()))?;
        tx.commit()?;
        drop(conn);
        Ok(Some(applied))
    }
}

/// The body of `apply`, inside its transaction; a branch an open Box works on is refused.
fn apply_in(tx: &Tx<'_>, now: Ts, d: Decision) -> Result<Applied, StoreError> {
    let branch = d.set.branch.clone();
    apply_steps(tx, now, d).map_err(|e| branch_taken(e, &branch))
}

/// Every write of `d`, inside its transaction.
fn apply_steps(tx: &Tx<'_>, now: Ts, mut d: Decision) -> Result<Applied, StoreError> {
    let steps::Stepped {
        id,
        existing,
        parts,
        from,
    } = step_box(tx, now, &d)?;
    if let (BoxTarget::New(new), true) = (&d.target, d.branch_by_id) {
        d.set.branch = Field::Set(moor_core::box_key::branch(id, &new.name));
    }
    let quiet = Step {
        from: from.map(is_quiet),
        to: is_quiet(parts),
    };
    if let (BoxTarget::New(_), Field::Set(branch)) = (&d.target, &d.set.branch) {
        tx.run(
            Query::insert()
                .into_table(BoxRefs::Table)
                .columns([BoxRefs::Box, BoxRefs::Kind, BoxRefs::Value])
                .values_panic([id.get().into(), "branch".into(), branch.into()])
                .on_conflict(OnConflict::new().do_nothing().to_owned()),
        )?;
    }
    let box_step = step_driver(tx, now, id, existing.as_ref(), &d)?;
    write_box_row(tx, id, now, &d, &quiet, parts)?;
    write_box(tx, id, now, &d, box_step.as_ref(), &quiet)?;
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
    if let (Some(i), Some(s)) = (&d.box_input, &box_step) {
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
    existing: Option<&DriverRow>,
    d: &Decision,
) -> Result<Option<Step<BoxState>>, StoreError> {
    let Some(input) = &d.box_input else {
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
    let to = BoxState::initial(input)?;
    let Field::Set(provider) = &d.set.box_provider else {
        return Err(StoreError::Invalid("new box needs box_provider".into()));
    };
    tx.run(
        box_update(id)
            .value(Boxes::Provider, provider.as_str())
            .value(Boxes::DriverState, to.as_str())
            .value(Boxes::DriverUpdatedAt, now.as_millis()),
    )?;
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
        sets.push((Boxes::State, lifecycle.as_str().into()));
        sets.extend(idle_since_sets(step, now));
        session.push((Sessions::State, session_state.as_str().into()));
        if session_state == SessionState::Ended {
            session.push((Sessions::EndedAt, now.as_millis().into()));
        }
    }
    sets.extend(intent_sets(d.intent, now));
    sets.push((Boxes::UpdatedAt, now.as_millis().into()));
    update(tx, &mut box_update(id), &sets)?;
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

/// `idle_since` follows quietness: stamped on becoming quiet, cleared on leaving it.
fn idle_since_sets(step: &Step<bool>, now: Ts) -> Sets<Boxes> {
    match (step.from, step.to) {
        (from, true) if from != Some(true) => {
            vec![(Boxes::IdleSince, now.as_millis().into())]
        }
        (Some(true), false) => vec![(Boxes::IdleSince, Value::BigInt(None))],
        _ => Vec::new(),
    }
}

/// Write box state, fields and handles; box fields without a box are invalid. A new handle is
/// a fresh disk, so the egress token delivered to the old one is delivered again.
///
/// A step out of quiet drops the recorded sleep refusal, so the next Idle stretch asks again.
fn write_box(
    tx: &Tx<'_>,
    id: BoxId,
    now: Ts,
    d: &Decision,
    step: Option<&Step<BoxState>>,
    quiet: &Step<bool>,
) -> Result<(), StoreError> {
    let mut sets = driver_field_sets(&d.set);
    if quiet.from == Some(true) && !quiet.to {
        if let Some(i) = sets.iter().position(|(c, _)| *c == Boxes::SleepReason) {
            sets.remove(i);
        }
        sets.push((Boxes::SleepReason, Value::String(None)));
    }
    let Some(step) = step else {
        if sets.is_empty() {
            return Ok(());
        }
        return Err(StoreError::Invalid(
            "Box fields set but the Box has no machine".into(),
        ));
    };
    if let Some(input) = &d.box_input {
        sets.push((Boxes::DriverState, step.to.as_str().into()));
        // A guest that may run again can change, so what was checked before no longer stands.
        if matches!(input, BoxInput::Wake | BoxInput::Acquired { .. }) {
            for column in [
                Boxes::SuspendAhead,
                Boxes::SuspendDirty,
                Boxes::SuspendCheckedAt,
                Boxes::SuspendPublish,
            ] {
                sets.push((column, None::<i64>.into()));
            }
        }
        sets.extend(input_handle_sets(input, &sets));
    }
    if d.box_input.is_some() {
        sets.push((Boxes::DriverUpdatedAt, now.as_millis().into()));
    }
    if sets.iter().any(|(c, _)| *c == Boxes::Handle) {
        super::proxy::forget_egress_delivery(tx, id)?;
    }
    update(tx, &mut box_update(id), &sets)
}

/// An UPDATE of Box `id`'s row.
fn box_update(id: BoxId) -> UpdateStatement {
    Query::update()
        .table(Boxes::Table)
        .and_where(Expr::col(Boxes::Id).eq(id.get()))
        .take()
}

/// Handles carried by a box input, unless the Fields already set that column.
fn input_handle_sets(input: &BoxInput, taken: &Sets<Boxes>) -> Sets<Boxes> {
    let (col, handle) = match input {
        BoxInput::Acquired { handle } => (Boxes::Handle, Some(handle)),
        BoxInput::AcquireFailed { attempted, .. } => (Boxes::AttemptedHandle, attempted.as_ref()),
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
pub(super) fn intent_sets(change: IntentChange, now: Ts) -> Sets<Boxes> {
    let (kind, pid, run, at) = match change {
        IntentChange::Keep => return Vec::new(),
        IntentChange::Clear => (None, None, None, None),
        IntentChange::Set(i) => (
            Some(i.kind().to_owned()),
            Some(i64::from(i.pid())),
            i.run().map(moor_core::RunId::get),
            Some(now.as_millis()),
        ),
    };
    vec![
        (Boxes::IntentKind, kind.into()),
        (Boxes::IntentPid, pid.into()),
        (Boxes::IntentRun, run.into()),
        (Boxes::IntentAt, at.into()),
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
