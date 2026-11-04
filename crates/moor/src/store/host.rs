//! Sessions that run on the host, with no Box.

use std::path::Path;

use moor_core::domain::session_state::{SessionInput, SessionState};
use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_schema::tables::Sessions;
use sea_query::{Expr, ExprTrait as _, Func, Query};

use super::{parsed_opt, Store, StoreError};
use crate::store::db::{Db, DbExt as _, OptionalExtension, TransactionBehavior};

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
fn load(conn: &dyn Db, id: i64) -> Result<Option<HostRow>, StoreError> {
    let raw = conn
        .row(
            Query::select()
                .columns([
                    Sessions::Workdir,
                    Sessions::Agent,
                    Sessions::AgentSessionId,
                    Sessions::State,
                    Sessions::EndedAt,
                ])
                .from(Sessions::Table)
                .and_where(Expr::col(Sessions::Id).eq(id))
                .and_where(Expr::col(Sessions::Box).is_null()),
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
fn load_live(conn: &dyn Db, id: i64) -> Result<HostRow, StoreError> {
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
            parent,
            user,
            key,
            ..
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
        let mut conn = self.writer();
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
        let id = insert(&tx, new, &given, state, now, user_id)?;
        let workdir = match workdir {
            HostWorkdir::At(_) => given,
            HostWorkdir::Under(base) => {
                let own = base.join(id.to_string()).display().to_string();
                tx.run(
                    Query::update()
                        .table(Sessions::Table)
                        .value(Sessions::Workdir, own.as_str())
                        .and_where(Expr::col(Sessions::Id).eq(id)),
                )?;
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
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        load_live(&tx, id)?;
        let mut update = Query::update();
        update
            .table(Sessions::Table)
            .and_where(Expr::col(Sessions::Id).eq(id));
        if let Some(a) = agent {
            update.value(Sessions::Agent, a);
        }
        if let Some(a) = agent_session_id {
            update.value(Sessions::AgentSessionId, a);
        }
        if agent.is_some() || agent_session_id.is_some() {
            tx.run(&mut update)?;
        }
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
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut row = load_live(&tx, id)?;
        row.state = row.state.step(&SessionInput::End)?;
        tx.run(
            Query::update()
                .table(Sessions::Table)
                .value(Sessions::State, row.state.as_str())
                .value(Sessions::EndedAt, now)
                .and_where(Expr::col(Sessions::Id).eq(id)),
        )?;
        tx.commit()?;
        drop(conn);
        row.ended_at = Some(now);
        Ok(row)
    }
}

/// Insert the host Session `new` working in `workdir`; its id.
fn insert(
    conn: &dyn Db,
    new: &NewHost<'_>,
    workdir: &str,
    state: SessionState,
    now: Ts,
    user: Option<i64>,
) -> Result<i64, StoreError> {
    Ok(conn.row(
        Query::insert()
            .into_table(Sessions::Table)
            .columns([
                Sessions::Box,
                Sessions::Ordinal,
                Sessions::State,
                Sessions::StartedAt,
                Sessions::Workdir,
                Sessions::Agent,
                Sessions::AgentSessionId,
                Sessions::ParentSession,
                Sessions::User,
                Sessions::HostKey,
            ])
            .values_panic([
                Option::<i64>::None.into(),
                1.into(),
                state.as_str().into(),
                now.as_millis().into(),
                workdir.into(),
                new.agent.into(),
                new.agent_session_id.into(),
                new.parent.into(),
                user.into(),
                new.key.into(),
            ])
            .returning_col(Sessions::Id),
        |r| r.get(0),
    )?)
}

/// Refuses parent `p` when it is unknown or has ended.
fn check_parent(conn: &dyn Db, p: i64) -> Result<(), StoreError> {
    let row: Option<(String, Option<i64>)> = conn
        .row(
            Query::select()
                .columns([Sessions::State, Sessions::EndedAt])
                .from(Sessions::Table)
                .and_where(Expr::col(Sessions::Id).eq(p)),
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
    conn: &dyn Db,
    user: Option<i64>,
    key: &str,
) -> Result<Option<(i64, String)>, StoreError> {
    Ok(conn
        .row(
            Query::select()
                .columns([Sessions::Id, Sessions::Workdir])
                .from(Sessions::Table)
                .and_where(Expr::col(Sessions::Box).is_null())
                .and_where(Expr::col(Sessions::HostKey).eq(key))
                .and_where(
                    Func::coalesce([Expr::col(Sessions::User), Expr::val(0)]).eq(user.unwrap_or(0)),
                )
                .and_where(Expr::col(Sessions::EndedAt).is_null()),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

#[cfg(test)]
mod tests;
