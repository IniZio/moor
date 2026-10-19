//! Steps a Box and its primary Session through their own machines for one decision.

use moor_core::domain::box_state::{BoxLifecycle, StepError};
use moor_core::domain::box_steps;
use moor_core::domain::decision::{Decision, NewTask, TaskTarget};
use moor_core::domain::session_state::SessionState;
use moor_core::domain::task::{TaskInput, TaskState};
use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use rusqlite::{params, Transaction};

use super::sql::name_taken;
use super::Step;
use crate::store::{reads::load_row_parts, sessions::live_parent, BoxRow, StoreError};

/// The Box and primary Session states a step leaves behind.
pub(super) type Parts = (BoxLifecycle, SessionState);

/// Resolve the Box (inserting a new one) and step its lifecycle and primary Session.
///
/// The `Step` carries the combined summary events use; the parts are what the writer stores.
pub(super) fn step_task(
    tx: &Transaction<'_>,
    now: Ts,
    d: &Decision,
) -> Result<(BoxId, Option<BoxRow>, Step<TaskState>, Parts), StoreError> {
    match &d.task {
        TaskTarget::New(new) => {
            let Some(input @ TaskInput::Insert) = &d.task_input else {
                return Err(StoreError::Invalid("new Box needs the insert input".into()));
            };
            let parts = box_steps::initial(input)?;
            let id = insert_task(tx, new, parts.0, d.parent_session, now)?;
            let to = box_steps::summary(parts.0, parts.1);
            Ok((id, None, Step { from: None, to }, parts))
        }
        TaskTarget::Existing(id) => {
            let (row, lifecycle, session) =
                load_row_parts(tx, *id)?.ok_or(StoreError::NotFound(*id))?;
            let from = (lifecycle, session);
            let parts = match &d.task_input {
                Some(i) => step_parts(from, i, tx, *id)?,
                None => from,
            };
            Ok((
                *id,
                row.boxed,
                Step {
                    from: Some(box_steps::summary(from.0, from.1)),
                    to: box_steps::summary(parts.0, parts.1),
                },
                parts,
            ))
        }
    }
}

/// Apply `input` to `from`, refusing a sleep while another Session is busy.
fn step_parts(
    from: Parts,
    input: &TaskInput,
    tx: &Transaction<'_>,
    id: BoxId,
) -> Result<Parts, StoreError> {
    let others = read_others(tx, id)?;
    box_steps::step(from.0, from.1, &others, input).map_err(|e| match e {
        StepError::Illegal(i) => StoreError::Illegal(i),
        StepError::SleepBlocked { busy } => StoreError::SleepBlocked { busy },
    })
}

/// States of the Box's Sessions other than the current one.
fn read_others(tx: &Transaction<'_>, id: BoxId) -> Result<Vec<SessionState>, StoreError> {
    let mut stmt = tx.prepare("SELECT state FROM sessions WHERE box = ?1 AND ordinal < (SELECT MAX(ordinal) FROM sessions WHERE box = ?1)")?;
    let rows = stmt.query_map(params![id.get()], |r| r.get::<_, String>(0))?;
    rows.map(|s| {
        let s = s?;
        SessionState::parse(&s)
            .ok_or_else(|| StoreError::Invalid(format!("unknown Session state {s}")))
    })
    .collect()
}

/// Insert a fresh Box row with its primary Session.
pub(in crate::store) fn insert_task(
    tx: &Transaction<'_>,
    new: &NewTask,
    lifecycle: BoxLifecycle,
    parent_session: Option<i64>,
    now: Ts,
) -> Result<BoxId, StoreError> {
    if let Some(p) = parent_session {
        live_parent(tx, p)?;
    }
    tx.execute(
        "INSERT INTO boxes (name, repo, state, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
        params![
            new.name.as_str(),
            new.repo,
            lifecycle.as_str(),
            now.as_millis(),
            now.as_millis()
        ],
    )
    .map_err(|e| name_taken(e, new))?;
    let id = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO sessions (box, ordinal, state, started_at, parent_session) VALUES (?, 1, 'starting', ?, ?)",
        params![id, now.as_millis(), parent_session],
    )?;
    Ok(BoxId::new(id))
}
