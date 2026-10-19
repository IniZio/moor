//! Steps a Box and its primary Session through their own machines for one decision.

use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput, StepError};
use moor_core::domain::box_steps;
use moor_core::domain::decision::{Decision, NewTask, TaskTarget};
use moor_core::domain::session_state::{SessionInput, SessionState};
use moor_core::domain::task::{TaskInput, TaskState};
use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use rusqlite::{params, Transaction};

use super::sql::name_taken;
use super::Step;
use crate::store::{reads::load_row_parts, sessions::live_parent, BoxRow, StoreError};

/// The Box and primary Session states a step leaves behind.
pub(super) type Parts = (BoxLifecycle, SessionState);

/// A decision resolved against its Box: the row, the stepped states and the lifecycle it left.
pub(super) struct Stepped {
    /// The Box written.
    pub(super) id: BoxId,
    /// The Box row before the step, if it existed.
    pub(super) existing: Option<BoxRow>,
    /// The combined state callers still read.
    pub(super) summary: Step<TaskState>,
    /// What the writer stores.
    pub(super) parts: Parts,
    /// The lifecycle before the step; `None` for a fresh Box.
    pub(super) from_lifecycle: Option<BoxLifecycle>,
}

/// The lifecycle and Session inputs a decision carries, whichever way it was built.
pub(super) fn inputs(d: &Decision) -> (Option<BoxLifecycleInput>, Option<SessionInput>) {
    d.task_input.as_ref().map_or_else(
        || (d.lifecycle_input.clone(), d.session_input.clone()),
        TaskInput::pair,
    )
}

/// Resolve the Box (inserting a new one) and step its lifecycle and primary Session.
///
/// The summary is the combined state callers still read; the parts are what the writer stores.
pub(super) fn step_task(
    tx: &Transaction<'_>,
    now: Ts,
    d: &Decision,
) -> Result<Stepped, StoreError> {
    let (l, s) = inputs(d);
    match &d.task {
        TaskTarget::New(new) => {
            let Some(BoxLifecycleInput::Insert) = &l else {
                return Err(StoreError::Invalid("new Box needs the insert input".into()));
            };
            let parts = (
                BoxLifecycle::initial(&BoxLifecycleInput::Insert)?,
                SessionState::initial(&SessionInput::Open)?,
            );
            let id = insert_task(tx, new, parts.0, d.parent_session, now)?;
            let to = box_steps::summary(parts.0, parts.1);
            Ok(Stepped {
                id,
                existing: None,
                summary: Step { from: None, to },
                parts,
                from_lifecycle: None,
            })
        }
        TaskTarget::Existing(id) => {
            let (row, lifecycle, session) =
                load_row_parts(tx, *id)?.ok_or(StoreError::NotFound(*id))?;
            let parts = if l.is_some() || s.is_some() {
                let others = read_others(tx, *id)?;
                box_steps::apply(lifecycle, session, &others, l.as_ref(), s.as_ref()).map_err(
                    |e| match e {
                        StepError::Illegal(i) => StoreError::Illegal(i),
                        StepError::SleepBlocked { busy } => StoreError::SleepBlocked { busy },
                    },
                )?
            } else {
                (lifecycle, session)
            };
            Ok(Stepped {
                id: *id,
                existing: row.boxed,
                summary: Step {
                    from: Some(box_steps::summary(lifecycle, session)),
                    to: box_steps::summary(parts.0, parts.1),
                },
                parts,
                from_lifecycle: Some(lifecycle),
            })
        }
    }
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
