//! The only writer of task/box state and events: `Store::apply` plus run records.

use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, IntentChange, NewTask, TaskTarget};
use moor_core::domain::event::{box_events, task_events, Event};
use moor_core::domain::task::{TaskInput, TaskState};
use moor_core::seam::clock::Ts;
use moor_core::TaskId;
use rusqlite::types::Value;
use rusqlite::{params, Transaction, TransactionBehavior};

use super::{load_task, BoxRow, Store, StoreError};

/// Result of a committed `apply`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Applied {
    /// The task written.
    pub(crate) task: TaskId,
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

/// Column assignments for one UPDATE.
type Sets = Vec<(&'static str, Value)>;

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
}

/// The body of `apply`, inside its transaction.
fn apply_in(tx: &Transaction<'_>, now: Ts, d: Decision) -> Result<Applied, StoreError> {
    let (id, existing, task_step) = step_task(tx, now, &d)?;
    let box_step = step_box(tx, now, id, existing.as_ref(), &d)?;
    write_task(tx, id, now, &d, &task_step)?;
    write_box(tx, id, now, &d, box_step.as_ref())?;
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

/// Resolve the task (inserting a new one) and step its state.
fn step_task(
    tx: &Transaction<'_>,
    now: Ts,
    d: &Decision,
) -> Result<(TaskId, Option<BoxRow>, Step<TaskState>), StoreError> {
    match &d.task {
        TaskTarget::New(new) => {
            let Some(input @ TaskInput::Insert) = &d.task_input else {
                return Err(StoreError::Invalid(
                    "new task needs the insert input".into(),
                ));
            };
            let to = TaskState::initial(input)?;
            let id = insert_task(tx, new, to, now)?;
            Ok((id, None, Step { from: None, to }))
        }
        TaskTarget::Existing(id) => {
            let row = load_task(tx, *id)?.ok_or(StoreError::NotFound(*id))?;
            let to = match &d.task_input {
                Some(i) => row.state.step(i)?,
                None => row.state,
            };
            Ok((
                *id,
                row.boxed,
                Step {
                    from: Some(row.state),
                    to,
                },
            ))
        }
    }
}

/// Insert a fresh task row.
fn insert_task(
    tx: &Transaction<'_>,
    new: &NewTask,
    state: TaskState,
    now: Ts,
) -> Result<TaskId, StoreError> {
    tx.execute(
        "INSERT INTO tasks (name, repo, state, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
        params![
            new.name.as_str(),
            new.repo,
            state.as_str(),
            now.as_millis(),
            now.as_millis()
        ],
    )?;
    Ok(TaskId::new(tx.last_insert_rowid()))
}

/// Step the box state, creating the box row on the first input.
fn step_box(
    tx: &Transaction<'_>,
    now: Ts,
    id: TaskId,
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
        "INSERT INTO boxes (task, provider, state, updated_at) VALUES (?, ?, ?, ?)",
        params![id.get(), provider.as_str(), to.as_str(), now.as_millis()],
    )?;
    Ok(Some(Step { from: None, to }))
}

/// Write task state, fields and intent.
fn write_task(
    tx: &Transaction<'_>,
    id: TaskId,
    now: Ts,
    d: &Decision,
    step: &Step<TaskState>,
) -> Result<(), StoreError> {
    let mut sets = task_field_sets(&d.set);
    if d.task_input.is_some() {
        sets.push(("state", Value::Text(step.to.as_str().to_owned())));
    }
    sets.extend(intent_sets(d.intent, now));
    sets.push(("updated_at", Value::Integer(now.as_millis())));
    update(tx, "tasks", "id", id.get(), &sets)
}

/// Write box state, fields and handles; box fields without a box are invalid.
fn write_box(
    tx: &Transaction<'_>,
    id: TaskId,
    now: Ts,
    d: &Decision,
    step: Option<&Step<BoxState>>,
) -> Result<(), StoreError> {
    let mut sets = box_field_sets(&d.set);
    let Some(step) = step else {
        if sets.is_empty() {
            return Ok(());
        }
        return Err(StoreError::Invalid(
            "box fields set but the task has no box".into(),
        ));
    };
    if let Some(input) = &d.box_input {
        sets.push(("state", Value::Text(step.to.as_str().to_owned())));
        sets.extend(input_handle_sets(input, &sets));
    }
    sets.push(("updated_at", Value::Integer(now.as_millis())));
    update(tx, "boxes", "task", id.get(), &sets)
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

/// Map a `Field` to a column value: `None` keeps, `Null` clears.
fn field_value<T>(f: &Field<T>, conv: impl Fn(&T) -> Value) -> Option<Value> {
    match f {
        Field::Keep => None,
        Field::Set(v) => Some(conv(v)),
        Field::Clear => Some(Value::Null),
    }
}

/// Text field to column value.
fn text(f: &Field<String>) -> Option<Value> {
    field_value(f, |s| Value::Text(s.clone()))
}

/// Bool field to column value.
fn flag(f: &Field<bool>) -> Option<Value> {
    field_value(f, |b| Value::Integer(i64::from(*b)))
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
        ("workspace_id", text(&s.workspace_id)),
        ("agent_session_id", text(&s.agent_session_id)),
        ("state_reason", text(&s.state_reason)),
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
        ("egress_enforced", flag(&s.egress_enforced)),
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

/// Run `UPDATE table SET .. WHERE key = ?` with static column names.
pub(super) fn update(
    tx: &Transaction<'_>,
    table: &str,
    key: &str,
    id: i64,
    sets: &Sets,
) -> Result<(), StoreError> {
    if sets.is_empty() {
        return Ok(());
    }
    let cols = sets
        .iter()
        .map(|(c, _)| format!("{c} = ?"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut args: Vec<Value> = sets.iter().map(|(_, v)| v.clone()).collect();
    args.push(Value::Integer(id));
    tx.execute(
        &format!("UPDATE {table} SET {cols} WHERE {key} = ?"),
        rusqlite::params_from_iter(args),
    )?;
    Ok(())
}

/// Insert one event row and return its id.
pub(super) fn insert_event(
    tx: &Transaction<'_>,
    task: TaskId,
    e: &Event,
    now: Ts,
) -> Result<i64, StoreError> {
    tx.execute(
        "INSERT INTO events (task, kind, payload, ts) VALUES (?, ?, ?, ?)",
        params![
            task.get(),
            e.kind(),
            e.payload().to_string(),
            now.as_millis()
        ],
    )?;
    Ok(tx.last_insert_rowid())
}
