//! Sessions of a Box beyond the primary one inserted with it.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::event::Event;
use moor_core::domain::session_state::{SessionInput, SessionState};
use moor_core::BoxId;
use moor_schema::tables::{Boxes, Launches, Sessions};
use sea_query::{Expr, ExprTrait as _, Func, Order, Query};

use super::apply::insert_event;
use super::{parsed_opt, Store, StoreError};
use crate::store::db::{Db, DbExt as _, OptionalExtension};

mod read;

pub(crate) use read::SessionRecord;

/// One Session of a Box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoxSession {
    /// Row id of the Session.
    pub(crate) id: i64,
    /// Position among the Box's Sessions, starting at 1.
    pub(crate) ordinal: i64,
    /// Where the Session is in its life.
    pub(crate) state: SessionState,
}

/// Succeed when Session `parent` exists and has not ended, reading through `conn`.
///
/// Call it inside a write transaction so the parent cannot end between the check and the insert.
///
/// # Errors
/// Returns [`StoreError::Invalid`] when the Session is unknown or ended, or [`StoreError`] when the database fails.
pub(in crate::store) fn live_parent(conn: &dyn Db, parent: i64) -> Result<(), StoreError> {
    let row: Option<(String, Option<i64>)> = conn
        .row(
            Query::select()
                .columns([Sessions::State, Sessions::EndedAt])
                .from(Sessions::Table)
                .and_where(Expr::col(Sessions::Id).eq(parent)),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    match row {
        None => Err(StoreError::Invalid(format!("no Session {parent}"))),
        Some((state, ended_at)) if state == SessionState::Ended.as_str() || ended_at.is_some() => {
            Err(StoreError::Invalid(format!("Session {parent} has ended")))
        }
        Some(_) => Ok(()),
    }
}

/// Whether Box `id` exists, reading through `conn`.
fn box_exists(conn: &dyn Db, id: BoxId) -> Result<bool, StoreError> {
    Ok(conn
        .row(
            Query::select()
                .column(Boxes::Id)
                .from(Boxes::Table)
                .and_where(Expr::col(Boxes::Id).eq(id.get())),
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// The state, ordinal and workspace of Box `id`'s newest Session, if it has one.
fn newest_of(
    conn: &dyn Db,
    id: BoxId,
) -> Result<Option<(String, i64, Option<String>)>, StoreError> {
    Ok(conn
        .row(
            Query::select()
                .columns([Sessions::State, Sessions::Ordinal, Sessions::WorkspaceId])
                .from(Sessions::Table)
                .and_where(Expr::col(Sessions::Box).eq(id.get()))
                .order_by(Sessions::Ordinal, Order::Desc)
                .limit(1),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?)
}

/// Stamp `idle_since` on Box `id` if it is Ready and has none, and mark it updated at `at`.
fn stamp_idle(conn: &dyn Db, id: BoxId, at: i64) -> Result<(), StoreError> {
    let idle_since = Expr::case(
        Expr::col(Boxes::State).eq(BoxLifecycle::Ready.as_str()),
        Func::coalesce([Expr::col(Boxes::IdleSince), Expr::val(at)]),
    )
    .finally(Expr::col(Boxes::IdleSince));
    conn.run(
        Query::update()
            .table(Boxes::Table)
            .value(Boxes::IdleSince, idle_since)
            .value(Boxes::UpdatedAt, at)
            .and_where(Expr::col(Boxes::Id).eq(id.get())),
    )?;
    Ok(())
}

impl Store {
    /// Open a Session on Box `id` with no parent.
    #[cfg(test)]
    pub(crate) fn open_session(&self, id: BoxId) -> Result<BoxSession, StoreError> {
        self.open_child_session(id, None)
    }

    /// Open another Session on Box `id`, numbered after its last one, in the Starting state,
    /// recording `parent` as its parent Session. It keeps the Box's workspace from the last one.
    ///
    /// # Errors
    /// Returns [`StoreError::NotFound`] when the Box does not exist, [`StoreError::Invalid`] when its current Session is not Ended, or [`StoreError`] when the database fails.
    pub(crate) fn open_child_session(
        &self,
        id: BoxId,
        parent: Option<i64>,
    ) -> Result<BoxSession, StoreError> {
        let now = self.now();
        let mut conn = self.writer();
        let tx = conn.immediate()?;
        if !box_exists(&tx, id)? {
            return Err(StoreError::NotFound(id));
        }
        let newest = newest_of(&tx, id)?;
        if newest
            .as_ref()
            .is_some_and(|(state, ..)| state != SessionState::Ended.as_str())
        {
            return Err(StoreError::Invalid(format!(
                "Box {} already has a live Session",
                id.get()
            )));
        }
        let (ordinal, workspace) = newest.map_or((1, None), |(_, ordinal, workspace)| {
            (ordinal + 1, workspace)
        });
        if let Some(p) = parent {
            live_parent(&tx, p)?;
        }
        let state = SessionState::Starting;
        let session = BoxSession {
            id: tx.row(
                Query::insert()
                    .into_table(Sessions::Table)
                    .columns([
                        Sessions::Box,
                        Sessions::Ordinal,
                        Sessions::State,
                        Sessions::StartedAt,
                        Sessions::ParentSession,
                        Sessions::WorkspaceId,
                    ])
                    .values_panic([
                        id.get().into(),
                        ordinal.into(),
                        state.as_str().into(),
                        now.as_millis().into(),
                        parent.into(),
                        workspace.into(),
                    ])
                    .returning_col(Sessions::Id),
                |r| r.get(0),
            )?,
            ordinal,
            state,
        };
        tx.commit()?;
        drop(conn);
        Ok(session)
    }

    /// Succeed when Session `parent` exists and has not ended.
    ///
    /// # Errors
    /// Returns [`StoreError::Invalid`] when the Session is unknown or ended, or [`StoreError`] when the database fails.
    pub(crate) fn check_parent(&self, parent: i64) -> Result<(), StoreError> {
        live_parent(&self.reader()?, parent)
    }

    /// Move a Starting Session to Working, the way a Box's first Session does once its agent runs.
    ///
    /// # Errors
    /// Returns [`StoreError::Illegal`] when the Session is not Starting, or [`StoreError`] when the database fails.
    pub(crate) fn start_session(&self, session: i64) -> Result<(), StoreError> {
        let mut conn = self.writer();
        let tx = conn.immediate()?;
        let state: String = tx.row(
            Query::select()
                .column(Sessions::State)
                .from(Sessions::Table)
                .and_where(Expr::col(Sessions::Id).eq(session)),
            |row| row.get(0),
        )?;
        let to = parsed_opt("session state", &state, SessionState::parse)?
            .step(&SessionInput::Started)?;
        tx.run(
            Query::update()
                .table(Sessions::Table)
                .value(Sessions::State, to.as_str())
                .and_where(Expr::col(Sessions::Id).eq(session)),
        )?;
        tx.commit()?;
        drop(conn);
        Ok(())
    }

    /// End Box `id`'s current Session and its linked launch in one transaction.
    ///
    /// Returns `false`, writing nothing, when the Box has no Session or its current one already ended.
    /// A Ready Box with an Ended Session reads as Idle, so `idle_since` is stamped when unset.
    ///
    /// # Errors
    /// Returns [`StoreError::NotFound`] when the Box does not exist, [`StoreError::Illegal`] when the
    /// Session cannot end, or [`StoreError`] when the database fails.
    pub(crate) fn end_current_session(
        &self,
        id: BoxId,
        cause: &str,
        exit: Option<i32>,
    ) -> Result<bool, StoreError> {
        let now = self.now();
        let mut conn = self.writer();
        let tx = conn.immediate()?;
        if !box_exists(&tx, id)? {
            return Err(StoreError::NotFound(id));
        }
        let current: Option<(i64, String)> = tx
            .row(
                Query::select()
                    .columns([Sessions::Id, Sessions::State])
                    .from(Sessions::Table)
                    .and_where(Expr::col(Sessions::Box).eq(id.get()))
                    .order_by(Sessions::Ordinal, Order::Desc)
                    .limit(1),
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((session, state)) = current else {
            return Ok(false);
        };
        let state = parsed_opt("session state", &state, SessionState::parse)?;
        if state == SessionState::Ended {
            return Ok(false);
        }
        let ended = state.step(&SessionInput::End)?;
        let at = now.as_millis();
        tx.run(
            Query::update()
                .table(Sessions::Table)
                .value(Sessions::State, ended.as_str())
                .value(Sessions::EndedAt, at)
                .and_where(Expr::col(Sessions::Id).eq(session)),
        )?;
        tx.run(
            Query::update()
                .table(Launches::Table)
                .value(Launches::EndedAt, at)
                .value(Launches::Exit, exit)
                .and_where(Expr::col(Launches::Session).eq(session))
                .and_where(Expr::col(Launches::EndedAt).is_null()),
        )?;
        stamp_idle(&tx, id, at)?;
        let event = Event::SessionEnded {
            session,
            cause: cause.to_owned(),
            exit,
        };
        insert_event(&tx, id, &event, now)?;
        tx.commit()?;
        drop(conn);
        Ok(true)
    }

    /// Every Session of Box `id`, by ordinal.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails or a stored state is unknown.
    pub(crate) fn box_sessions(&self, id: BoxId) -> Result<Vec<BoxSession>, StoreError> {
        let conn = self.reader()?;
        let rows = conn.rows(
            Query::select()
                .columns([Sessions::Id, Sessions::Ordinal, Sessions::State])
                .from(Sessions::Table)
                .and_where(Expr::col(Sessions::Box).eq(id.get()))
                .order_by(Sessions::Ordinal, Order::Asc),
            |row| {
                Ok::<_, StoreError>((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?;
        rows.map(|row| {
            let (id, ordinal, state) = row?;
            Ok(BoxSession {
                id,
                ordinal,
                state: parsed_opt("session state", &state, SessionState::parse)?,
            })
        })
        .collect()
    }
}

#[cfg(test)]
mod tests;
