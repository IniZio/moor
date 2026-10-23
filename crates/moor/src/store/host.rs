//! Sessions that run on the host, with no Box.

use std::path::Path;

use moor_core::domain::session_state::{SessionInput, SessionState};
use moor_core::user::UserId;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

use super::{parsed_opt, Store, StoreError};

/// One host Session row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostRow {
    /// Session id.
    pub(crate) id: i64,
    /// Directory the agent runs in.
    pub(crate) workdir: String,
    /// Agent name, when known.
    pub(crate) agent: Option<String>,
    /// The agent's own session id, when known.
    pub(crate) agent_session_id: Option<String>,
    /// Current state.
    pub(crate) state: SessionState,
    /// When the Session ended, in milliseconds.
    pub(crate) ended_at: Option<i64>,
}

/// Read the host Session `id`; Box Sessions are not host rows.
fn load(conn: &Connection, id: i64) -> Result<Option<HostRow>, StoreError> {
    let raw = conn
        .query_row(
            "SELECT workdir, agent, agent_session_id, state, ended_at FROM sessions \
             WHERE id = ?1 AND box IS NULL",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((workdir, agent, agent_session_id, state, ended_at)) = raw else {
        return Ok(None);
    };
    let state = parsed_opt("session state", &state, SessionState::parse)?;
    Ok(Some(HostRow {
        id,
        workdir,
        agent,
        agent_session_id,
        state,
        ended_at,
    }))
}

/// Read a host Session that still runs, or say why it cannot change.
fn load_live(conn: &Connection, id: i64) -> Result<HostRow, StoreError> {
    let row =
        load(conn, id)?.ok_or_else(|| StoreError::Invalid(format!("no host Session {id}")))?;
    if row.ended_at.is_some() || row.state == SessionState::Ended {
        return Err(StoreError::Invalid(format!("host Session {id} has ended")));
    }
    Ok(row)
}

/// Where a new host Session runs.
#[derive(Debug, Clone, Copy)]
pub(crate) enum HostWorkdir<'a> {
    /// The directory the caller chose.
    At(&'a str),
    /// A directory of its own, named by the Session's id, inside this one.
    Under(&'a Path),
}

/// A host Session to open.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NewHost<'a> {
    /// Where it runs.
    pub(crate) workdir: HostWorkdir<'a>,
    /// Agent it runs.
    pub(crate) agent: Option<&'a str>,
    /// The agent's own id for the Session.
    pub(crate) agent_session_id: Option<&'a str>,
    /// Row id of the parent Session.
    pub(crate) parent: Option<i64>,
    /// The user it belongs to; `None` for the operator's.
    pub(crate) user: Option<UserId>,
    /// The opener's own name for it; a live Session of the same user and key is returned.
    pub(crate) key: Option<&'a str>,
}

impl Store {
    /// Register a Session that runs on the host in `workdir`, with no parent; returns its id.
    ///
    /// # Errors
    /// As [`Store::open_child_host_session`].
    #[cfg(test)]
    pub(crate) fn open_host_session(
        &self,
        workdir: &str,
        agent: Option<&str>,
        agent_session_id: Option<&str>,
    ) -> Result<i64, StoreError> {
        self.open_child_host_session(&NewHost {
            workdir: HostWorkdir::At(workdir),
            agent,
            agent_session_id,
            parent: None,
            user: None,
            key: None,
        })
        .map(|(id, _, _)| id)
    }

    /// Register a Session that runs on the host in `workdir`, with no Box, under `parent` when
    /// given; returns its id, its workdir, and whether this call created it.
    ///
    /// The Session is Working at once and records no events, since events belong to a Box. The
    /// parent is checked in the same transaction as the insert.
    ///
    /// # Errors
    /// Returns [`StoreError::Invalid`] when `workdir` is empty or `parent` is unknown or has
    /// ended, [`StoreError::Illegal`] when the Session cannot start, or [`StoreError`] when the
    /// database fails.
    pub(crate) fn open_child_host_session(
        &self,
        new: &NewHost<'_>,
    ) -> Result<(i64, String, bool), StoreError> {
        let NewHost {
            workdir,
            agent,
            agent_session_id,
            parent,
            user,
            key,
        } = *new;
        let given = match workdir {
            HostWorkdir::At(dir) => dir.to_owned(),
            HostWorkdir::Under(base) => base.display().to_string(),
        };
        if given.is_empty() {
            return Err(StoreError::Invalid(
                "a host Session needs a workdir".to_owned(),
            ));
        }
        let state = SessionState::initial(&SessionInput::Open)?.step(&SessionInput::Started)?;
        let now = self.now();
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(p) = parent {
            check_parent(&tx, p)?;
        }
        let user_id = user.map(UserId::get);
        if let Some((id, dir)) = key
            .map(|k| live_keyed(&tx, user_id, k))
            .transpose()?
            .flatten()
        {
            return Ok((id, dir, false));
        }
        tx.execute(
            "INSERT INTO sessions (box, ordinal, state, started_at, workdir, agent, agent_session_id, parent_session, user, host_key) \
             VALUES (NULL, 1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            (state.as_str(), now.as_millis(), &given, agent, agent_session_id, parent, user_id, key),
        )?;
        let id = tx.last_insert_rowid();
        let workdir = match workdir {
            HostWorkdir::At(_) => given,
            HostWorkdir::Under(base) => {
                let own = base.join(id.to_string()).display().to_string();
                tx.execute("UPDATE sessions SET workdir = ?1 WHERE id = ?2", (&own, id))?;
                own
            }
        };
        tx.commit()?;
        drop(conn);
        Ok((id, workdir, true))
    }

    /// Read the host Session `id`; `None` when no host Session has that id.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails or the stored state is unknown.
    #[cfg(test)]
    pub(crate) fn host_session(&self, id: i64) -> Result<Option<HostRow>, StoreError> {
        load(&self.reader()?, id)
    }

    /// Set the agent fields that are `Some` on a live host Session; `None` leaves a column as is.
    ///
    /// # Errors
    /// Returns [`StoreError::Invalid`] when no host Session has that id or it has ended, or
    /// [`StoreError`] when the database fails.
    pub(crate) fn update_host_session(
        &self,
        id: i64,
        agent: Option<&str>,
        agent_session_id: Option<&str>,
    ) -> Result<HostRow, StoreError> {
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        load_live(&tx, id)?;
        tx.execute(
            "UPDATE sessions SET agent = COALESCE(?2, agent), \
             agent_session_id = COALESCE(?3, agent_session_id) WHERE id = ?1",
            (id, agent, agent_session_id),
        )?;
        let row = load_live(&tx, id)?;
        tx.commit()?;
        drop(conn);
        Ok(row)
    }

    /// End a live host Session, recording when; records no events.
    ///
    /// # Errors
    /// Returns [`StoreError::Invalid`] when no host Session has that id or it has ended,
    /// [`StoreError::Illegal`] when its state cannot end, or [`StoreError`] when the database fails.
    pub(crate) fn end_host_session(&self, id: i64) -> Result<HostRow, StoreError> {
        let now = self.now().as_millis();
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut row = load_live(&tx, id)?;
        row.state = row.state.step(&SessionInput::End)?;
        tx.execute(
            "UPDATE sessions SET state = ?2, ended_at = ?3 WHERE id = ?1",
            (id, row.state.as_str(), now),
        )?;
        tx.commit()?;
        drop(conn);
        row.ended_at = Some(now);
        Ok(row)
    }
}

/// Refuses parent `p` when it is unknown or has ended.
fn check_parent(conn: &Connection, p: i64) -> Result<(), StoreError> {
    let row: Option<(String, Option<i64>)> = conn
        .query_row(
            "SELECT state, ended_at FROM sessions WHERE id = ?1",
            [p],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    match row {
        None => Err(StoreError::Invalid(format!("no Session {p}"))),
        Some((s, ended_at)) if s == SessionState::Ended.as_str() || ended_at.is_some() => {
            Err(StoreError::Invalid(format!("Session {p} has ended")))
        }
        Some(_) => Ok(()),
    }
}

/// The id and workdir of the live host Session that `user` (none for the operator) opened
/// with `key`.
fn live_keyed(
    conn: &Connection,
    user: Option<i64>,
    key: &str,
) -> Result<Option<(i64, String)>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT id, workdir FROM sessions WHERE box IS NULL AND host_key = ?1 \
             AND IFNULL(user, 0) = IFNULL(?2, 0) AND ended_at IS NULL",
            (key, user),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

#[cfg(test)]
mod tests;
