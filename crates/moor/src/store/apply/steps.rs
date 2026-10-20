//! Steps a Box and its primary Session through their own machines for one decision.

use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput, StepError};
use moor_core::domain::box_steps;
use moor_core::domain::decision::{BoxTarget, Decision, NewBox};
use moor_core::domain::session_state::{SessionInput, SessionState};
use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use rusqlite::{params, Transaction};

use super::sql::name_taken;
use crate::store::{load_box, sessions::live_parent, DriverRow, StoreError};

/// The Box and primary Session states a step leaves behind.
pub(super) type Parts = (BoxLifecycle, SessionState);

/// A decision resolved against its Box: the row, the stepped states and the states it left.
pub(super) struct Stepped {
    /// The Box written.
    pub(super) id: BoxId,
    /// The Box row before the step, if it existed.
    pub(super) existing: Option<DriverRow>,
    /// What the writer stores.
    pub(super) parts: Parts,
    /// The states before the step; `None` for a fresh Box.
    pub(super) from: Option<Parts>,
}

/// Resolve the Box (inserting a new one) and step its lifecycle and primary Session.
///
/// The parts are what the writer stores.
pub(super) fn step_box(tx: &Transaction<'_>, now: Ts, d: &Decision) -> Result<Stepped, StoreError> {
    let (l, s) = (d.lifecycle_input.as_ref(), d.session_input.as_ref());
    match &d.target {
        BoxTarget::New(new) => {
            let Some(BoxLifecycleInput::Insert) = l else {
                return Err(StoreError::Invalid("new Box needs the insert input".into()));
            };
            let parts = (
                BoxLifecycle::initial(&BoxLifecycleInput::Insert)?,
                SessionState::initial(&SessionInput::Open)?,
            );
            let id = insert_box(tx, new, parts.0, d.parent_session, now)?;
            Ok(Stepped {
                id,
                existing: None,
                parts,
                from: None,
            })
        }
        BoxTarget::Existing(id) => {
            let row = load_box(tx, *id)?.ok_or(StoreError::NotFound(*id))?;
            let (lifecycle, session) = (row.lifecycle, row.session);
            let parts = if l.is_some() || s.is_some() {
                let others = read_others(tx, *id)?;
                box_steps::apply(lifecycle, session, &others, l, s).map_err(|e| match e {
                    StepError::Illegal(i) => StoreError::Illegal(i),
                    StepError::SleepBlocked { busy } => StoreError::SleepBlocked { busy },
                })?
            } else {
                (lifecycle, session)
            };
            Ok(Stepped {
                id: *id,
                existing: row.boxed,
                parts,
                from: Some((lifecycle, session)),
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
pub(in crate::store) fn insert_box(
    tx: &Transaction<'_>,
    new: &NewBox,
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
