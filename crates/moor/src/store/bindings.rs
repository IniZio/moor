//! Bindings of a front conversation to a Session, host or Box.

use moor_core::BoxName;
use rusqlite::{params, OptionalExtension, TransactionBehavior};

use super::{parsed, Store, StoreError};

/// A front conversation bound to a Session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Binding {
    /// Front the conversation belongs to.
    pub(crate) front: String,
    /// The front's key for the conversation.
    pub(crate) key: String,
    /// Id of the bound Session.
    pub(crate) session: i64,
    /// Name of the Session's Box; `None` for a host Session.
    pub(crate) task: Option<BoxName>,
    /// The front's id of the user who started the conversation.
    pub(crate) owner: String,
    /// Timestamp of the edited status message, once posted.
    pub(crate) status_ts: Option<String>,
}

/// Columns read for a [`Binding`], in [`BindingRow`] order.
const COLS: &str = "g.front, g.conv_key, g.session, b.name, g.owner, g.status_ts";

/// Joins a binding to its Session and, for a Box Session, the Box.
const FROM: &str =
    "bindings g JOIN sessions s ON s.id = g.session LEFT JOIN boxes b ON b.id = s.box";

/// Raw binding row: the Box name is still unparsed text.
type BindingRow = (String, String, i64, Option<String>, String, Option<String>);

/// Reads one raw binding row.
fn row_of(r: &rusqlite::Row<'_>) -> rusqlite::Result<BindingRow> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
    ))
}

/// Parses a raw binding row into a [`Binding`].
fn binding_of(row: BindingRow) -> Result<Binding, StoreError> {
    let (front, key, session, task, owner, status_ts) = row;
    Ok(Binding {
        front,
        key,
        session,
        task: task
            .map(|n| parsed("binding task", &n, BoxName::parse_allow_bare))
            .transpose()?,
        owner,
        status_ts,
    })
}

/// Insert the binding unless (front, key) is already bound; true when this call created it.
pub(super) fn insert_binding(
    conn: &rusqlite::Connection,
    front: &str,
    key: &str,
    session: i64,
    owner: &str,
    at: i64,
) -> Result<bool, StoreError> {
    let added = conn.execute(
        "INSERT OR IGNORE INTO bindings (front, conv_key, session, owner, created_at) \
         VALUES (?, ?, ?, ?, ?)",
        params![front, key, session, owner, at],
    )?;
    Ok(added == 1)
}

impl Store {
    /// Binds (front, key) to Session `session`, owned by `owner`.
    /// The first binding wins; true when this call created it.
    /// Errors with [`StoreError::Invalid`] when the Session does not exist or has ended.
    pub(crate) fn bind_session(
        &self,
        front: &str,
        key: &str,
        session: i64,
        owner: &str,
    ) -> Result<bool, StoreError> {
        let now = self.now();
        let mut writer = self.writer()?;
        let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let state: Option<String> = tx
            .query_row(
                "SELECT state FROM sessions WHERE id = ?",
                params![session],
                |r| r.get(0),
            )
            .optional()?;
        match state.as_deref() {
            None => return Err(StoreError::Invalid(format!("no Session {session}"))),
            Some("ended") => {
                return Err(StoreError::Invalid(format!("Session {session} has ended")))
            }
            Some(_) => {}
        }
        let added = insert_binding(&tx, front, key, session, owner, now.as_millis())?;
        tx.commit()?;
        drop(writer);
        Ok(added)
    }

    /// The binding of (front, key), if any.
    pub(crate) fn binding(&self, front: &str, key: &str) -> Result<Option<Binding>, StoreError> {
        self.reader()?
            .query_row(
                &format!("SELECT {COLS} FROM {FROM} WHERE g.front = ? AND g.conv_key = ?"),
                params![front, key],
                row_of,
            )
            .optional()?
            .map(binding_of)
            .transpose()
    }

    /// All bindings of `front`, oldest first.
    pub(crate) fn bindings(&self, front: &str) -> Result<Vec<Binding>, StoreError> {
        let reader = self.reader()?;
        let mut stmt = reader.prepare(&format!(
            "SELECT {COLS} FROM {FROM} WHERE g.front = ? ORDER BY g.created_at, g.rowid"
        ))?;
        let rows = stmt
            .query_map(params![front], row_of)?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter().map(binding_of).collect()
    }
}

#[cfg(test)]
mod tests;
