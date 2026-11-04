//! Bindings of a front conversation to a Session, host or Box.

use moor_core::BoxName;
use moor_schema::tables::{Bindings, Boxes, Sessions};
use sea_query::{Expr, ExprTrait as _, OnConflict, Order, Query, SelectStatement};

use super::{parsed, Store, StoreError};
use crate::store::db::{Db, DbExt as _, OptionalExtension};

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
    pub(crate) box_name: Option<BoxName>,
    /// The front's id of the user who started the conversation.
    pub(crate) owner: String,
}

/// Whether the binding's Session has not ended.
fn live() -> Expr {
    Expr::col((Sessions::Table, Sessions::State)).ne("ended")
}

/// Bindings joined to their Session and, for a Box Session, the Box.
fn joined() -> SelectStatement {
    Query::select()
        .from(Bindings::Table)
        .inner_join(
            Sessions::Table,
            Expr::col((Sessions::Table, Sessions::Id)).equals((Bindings::Table, Bindings::Session)),
        )
        .left_join(
            Boxes::Table,
            Expr::col((Boxes::Table, Boxes::Id)).equals((Sessions::Table, Sessions::Box)),
        )
        .take()
}

/// [`joined`] with the columns of a [`Binding`], in [`BindingRow`] order.
fn binding_select() -> SelectStatement {
    let b = |c| (Bindings::Table, c);
    joined()
        .columns([
            b(Bindings::Front),
            b(Bindings::ConvKey),
            b(Bindings::Session),
        ])
        .column((Boxes::Table, Boxes::Name))
        .column(b(Bindings::Owner))
        .take()
}

/// Orders `q`'s bindings newest first.
fn newest(q: &mut SelectStatement) -> &mut SelectStatement {
    q.order_by((Bindings::Table, Bindings::CreatedAt), Order::Desc)
        .order_by((Bindings::Table, Bindings::Id), Order::Desc)
}

/// Raw binding row: the Box name is still unparsed text.
type BindingRow = (String, String, i64, Option<String>, String);

/// Reads one raw binding row.
fn row_of(r: &crate::store::db::Row<'_>) -> crate::store::db::Result<BindingRow> {
    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
}

/// Parses a raw binding row into a [`Binding`].
fn binding_of(row: BindingRow) -> Result<Binding, StoreError> {
    let (front, key, session, box_name, owner) = row;
    Ok(Binding {
        front,
        key,
        session,
        box_name: box_name
            .map(|n| parsed("binding box name", &n, BoxName::parse_allow_bare))
            .transpose()?,
        owner,
    })
}

/// Bind (front, key) to `session` unless the key's newest binding names a live Session;
/// true when this call added a row.
fn insert_binding(
    conn: &dyn Db,
    front: &str,
    key: &str,
    session: i64,
    owner: &str,
    at: i64,
) -> Result<bool, StoreError> {
    let mut newest_binding = joined();
    newest_binding
        .expr(live())
        .and_where(Expr::col((Bindings::Table, Bindings::Front)).eq(front))
        .and_where(Expr::col((Bindings::Table, Bindings::ConvKey)).eq(key));
    let newest_live: Option<bool> = conn
        .row(newest(&mut newest_binding).limit(1), |r| r.get(0))
        .optional()?;
    if newest_live == Some(true) {
        return Ok(false);
    }
    let added = conn.run(
        Query::insert()
            .into_table(Bindings::Table)
            .columns([
                Bindings::Front,
                Bindings::ConvKey,
                Bindings::Session,
                Bindings::Owner,
                Bindings::CreatedAt,
            ])
            .values_panic([
                front.into(),
                key.into(),
                session.into(),
                owner.into(),
                at.into(),
            ])
            .on_conflict(OnConflict::new().do_nothing().to_owned()),
    )?;
    Ok(added == 1)
}

impl Store {
    /// Binds (front, key) to Session `session`, owned by `owner`.
    /// While the key's newest binding names a live Session that binding wins; once that
    /// Session ends, a new Session binds beside it. True when this call created a binding.
    /// Errors with [`StoreError::Invalid`] when the Session does not exist or has ended.
    pub(crate) fn bind_session(
        &self,
        front: &str,
        key: &str,
        session: i64,
        owner: &str,
    ) -> Result<bool, StoreError> {
        let now = self.now();
        let mut writer = self.writer();
        let tx = writer.immediate()?;
        let state: Option<String> = tx
            .row(
                Query::select()
                    .column(Sessions::State)
                    .from(Sessions::Table)
                    .and_where(Expr::col(Sessions::Id).eq(session)),
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

    /// The binding of (front, key): the newest naming a live Session, else the newest.
    pub(crate) fn binding(&self, front: &str, key: &str) -> Result<Option<Binding>, StoreError> {
        let q = binding_select()
            .and_where(Expr::col((Bindings::Table, Bindings::Front)).eq(front))
            .and_where(Expr::col((Bindings::Table, Bindings::ConvKey)).eq(key))
            .order_by_expr(live(), Order::Desc)
            .order_by((Bindings::Table, Bindings::CreatedAt), Order::Desc)
            .order_by((Bindings::Table, Bindings::Id), Order::Desc)
            .limit(1)
            .take();
        self.reader()?
            .row(q, row_of)
            .optional()?
            .map(binding_of)
            .transpose()
    }

    /// The binding that names Session `session`, if any.
    #[cfg_attr(not(test), expect(dead_code, reason = "no caller outside tests yet"))]
    pub(crate) fn session_binding(&self, session: i64) -> Result<Option<Binding>, StoreError> {
        let mut q = binding_select();
        q.and_where(Expr::col((Bindings::Table, Bindings::Session)).eq(session));
        self.reader()?
            .row(newest(&mut q).limit(1).take(), row_of)
            .optional()?
            .map(binding_of)
            .transpose()
    }
}

#[cfg(test)]
mod tests;
