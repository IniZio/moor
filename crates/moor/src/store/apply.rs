//! The only writer of task/box state and events: `Store::apply` plus run records.

use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, IntentChange, TaskTarget};
use moor_core::domain::event::{box_events, task_events, Event};
use moor_core::domain::session_state::SessionState;
use moor_core::domain::task::TaskState;
use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use rusqlite::types::Value;
use rusqlite::{params, Transaction, TransactionBehavior};

mod sql;
mod steps;

pub(super) use self::sql::update;
use self::sql::{field_value, flag, text, Sets};
pub(super) use self::steps::insert_task;
use self::steps::{step_task, Parts};
use super::reads::live_holders_in;
use super::{BoxRow, Store, StoreError, TaskRow};
use moor_core::ProviderId;

/// Result of a committed `apply`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Applied {
    /// The task written.
    pub(crate) task: BoxId,
    /// Task state after the apply.
    pub(crate) task_state: TaskState,
    /// Box state after the apply, if the task has a box.
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
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let applied = apply_in(&tx, now, d)?;
        tx.commit()?;
        drop(conn);
        Ok(applied)
    }

    /// Read the live holders of `provider`, let `decide` pick a decision from them, and apply it.
    ///
    /// The read and the write share one transaction, so no other writer can claim a slot between them.
    /// `decide` returns `None` to write nothing. It runs under the writer lock, so it must not block.
    pub(crate) fn apply_admitting(
        &self,
        provider: &ProviderId,
        decide: impl FnOnce(Vec<TaskRow>) -> Option<Decision>,
    ) -> Result<Option<Applied>, StoreError> {
        let now = self.now();
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(d) = decide(live_holders_in(&tx, provider)?) else {
            return Ok(None);
        };
        let applied = apply_in(&tx, now, d)?;
        tx.commit()?;
        drop(conn);
        Ok(Some(applied))
    }
}

/// The body of `apply`, inside its transaction.
fn apply_in(tx: &Transaction<'_>, now: Ts, d: Decision) -> Result<Applied, StoreError> {
    let (id, existing, task_step, parts) = step_task(tx, now, &d)?;
    if let (TaskTarget::New(_), Field::Set(branch)) = (&d.task, &d.set.branch) {
        tx.execute(
            "INSERT INTO box_refs (box, kind, value) VALUES (?1, 'branch', ?2) ON CONFLICT DO NOTHING",
            params![id.get(), branch],
        )?;
    }
    let box_step = step_box(tx, now, id, existing.as_ref(), &d)?;
    write_task(tx, id, now, &d, &task_step, parts)?;
    write_box(tx, id, now, &d, box_step.as_ref(), &task_step)?;
    let mut events = Vec::new();
    if let Some(i) = &d.task_input {
        events.extend(task_events(task_step.from, task_step.to, i));
    }
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
    let ids = events
        .iter()
        .map(|e| insert_event(tx, id, e, now))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Applied {
        task: id,
        task_state: task_step.to,
        box_state: box_step.map(|s| s.to),
        events: ids,
    })
}

/// Step the box state, recording the provider on the first input.
fn step_box(
    tx: &Transaction<'_>,
    now: Ts,
    id: BoxId,
    existing: Option<&BoxRow>,
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
    tx.execute(
        "UPDATE boxes SET provider = ?, driver_state = ?, driver_updated_at = ? WHERE id = ?",
        params![provider.as_str(), to.as_str(), now.as_millis(), id.get()],
    )?;
    Ok(Some(Step { from: None, to }))
}

/// Write task state, fields and intent.
fn write_task(
    tx: &Transaction<'_>,
    id: BoxId,
    now: Ts,
    d: &Decision,
    step: &Step<TaskState>,
    (lifecycle, session_state): Parts,
) -> Result<(), StoreError> {
    let mut sets = task_field_sets(&d.set);
    let mut session = session_field_sets(&d.set);
    if d.task_input.is_some() {
        sets.push(("state", Value::Text(lifecycle.as_str().to_owned())));
        sets.extend(idle_since_sets(step, now));
        session.push(("state", Value::Text(session_state.as_str().to_owned())));
        if session_state == SessionState::Ended {
            session.push(("ended_at", Value::Integer(now.as_millis())));
        }
    }
    sets.extend(intent_sets(d.intent, now));
    sets.push(("updated_at", Value::Integer(now.as_millis())));
    update(tx, "boxes", "id = ?", &[id.get()], &sets)?;
    update(
        tx,
        "sessions",
        "box = ? AND ordinal = 1",
        &[id.get()],
        &session,
    )?;
    if d.task_input.is_some() && !step.to.is_live() {
        tx.execute(
            "UPDATE sessions SET state = 'ended', ended_at = ? WHERE box = ? AND state != 'ended'",
            params![now.as_millis(), id.get()],
        )?;
    }
    Ok(())
}

/// `idle_since` follows the state: stamped on entering Idle, cleared on leaving it.
fn idle_since_sets(step: &Step<TaskState>, now: Ts) -> Sets {
    match (step.from, step.to) {
        (from, TaskState::Idle) if from != Some(TaskState::Idle) => {
            vec![("idle_since", Value::Integer(now.as_millis()))]
        }
        (Some(TaskState::Idle), to) if to != TaskState::Idle => {
            vec![("idle_since", Value::Null)]
        }
        _ => Vec::new(),
    }
}

/// Write box state, fields and handles; box fields without a box are invalid.
///
/// A task step out of Idle drops the recorded sleep refusal, so the next Idle stretch asks again.
fn write_box(
    tx: &Transaction<'_>,
    id: BoxId,
    now: Ts,
    d: &Decision,
    step: Option<&Step<BoxState>>,
    task: &Step<TaskState>,
) -> Result<(), StoreError> {
    let mut sets = box_field_sets(&d.set);
    if task.from == Some(TaskState::Idle) && task.to != TaskState::Idle {
        if let Some(i) = sets.iter().position(|(c, _)| *c == "sleep_reason") {
            sets.remove(i);
        }
        sets.push(("sleep_reason", Value::Null));
    }
    let Some(step) = step else {
        if sets.is_empty() {
            return Ok(());
        }
        return Err(StoreError::Invalid(
            "box fields set but the task has no box".into(),
        ));
    };
    if let Some(input) = &d.box_input {
        sets.push(("driver_state", Value::Text(step.to.as_str().to_owned())));
        sets.extend(input_handle_sets(input, &sets));
    }
    sets.push(("driver_updated_at", Value::Integer(now.as_millis())));
    update(tx, "boxes", "id = ?", &[id.get()], &sets)
}

/// Handles carried by a box input, unless the Fields already set that column.
fn input_handle_sets(input: &BoxInput, taken: &Sets) -> Sets {
    let (col, handle) = match input {
        BoxInput::Acquired { handle } => ("handle", Some(handle)),
        BoxInput::AcquireFailed { attempted, .. } => ("attempted_handle", attempted.as_ref()),
        _ => return Vec::new(),
    };
    match handle {
        Some(h) if !taken.iter().any(|(c, _)| *c == col) => {
            vec![(col, Value::Text(h.as_str().to_owned()))]
        }
        _ => Vec::new(),
    }
}

/// Column assignments for the task fields.
fn task_field_sets(s: &Fields) -> Sets {
    let all = [
        ("issue", text(&s.issue)),
        ("goal", text(&s.goal)),
        ("base", text(&s.base)),
        ("branch", text(&s.branch)),
        ("worktree", text(&s.worktree)),
        ("bound", flag(&s.bound)),
        ("state_reason", text(&s.state_reason)),
        (
            "setup_failed_at",
            field_value(&s.setup_failed_at, |t| Value::Integer(*t)),
        ),
        ("launch_failure", text(&s.launch_failure)),
    ];
    all.into_iter().filter_map(|(c, v)| Some((c, v?))).collect()
}

/// Column assignments for the primary session fields.
fn session_field_sets(s: &Fields) -> Sets {
    let all = [
        ("workspace_id", text(&s.workspace_id)),
        ("agent_session_id", text(&s.agent_session_id)),
    ];
    all.into_iter().filter_map(|(c, v)| Some((c, v?))).collect()
}

/// Column assignments for the box fields.
fn box_field_sets(s: &Fields) -> Sets {
    let all = [
        (
            "provider",
            field_value(&s.box_provider, |p| Value::Text(p.as_str().to_owned())),
        ),
        (
            "handle",
            field_value(&s.box_handle, |h| Value::Text(h.as_str().to_owned())),
        ),
        ("workdir", text(&s.box_workdir)),
        ("egress", text(&s.egress)),
        ("sleep_reason", text(&s.box_sleep_reason)),
    ];
    all.into_iter().filter_map(|(c, v)| Some((c, v?))).collect()
}

/// Column assignments for an intent change.
pub(super) fn intent_sets(change: IntentChange, now: Ts) -> Sets {
    let (kind, pid, run, at) = match change {
        IntentChange::Keep => return Vec::new(),
        IntentChange::Clear => (Value::Null, Value::Null, Value::Null, Value::Null),
        IntentChange::Set(i) => (
            Value::Text(i.kind().to_owned()),
            Value::Integer(i64::from(i.pid())),
            i.run().map_or(Value::Null, |r| Value::Integer(r.get())),
            Value::Integer(now.as_millis()),
        ),
    };
    vec![
        ("intent_kind", kind),
        ("intent_pid", pid),
        ("intent_run", run),
        ("intent_at", at),
    ]
}

/// Insert one event row and return its id.
pub(super) fn insert_event(
    tx: &Transaction<'_>,
    task: BoxId,
    e: &Event,
    now: Ts,
) -> Result<i64, StoreError> {
    tx.execute(
        "INSERT INTO events (box, kind, payload, ts) VALUES (?, ?, ?, ?)",
        params![
            task.get(),
            e.kind(),
            e.payload().to_string(),
            now.as_millis()
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

#[cfg(test)]
mod tests;
