//! OAuth sign-ins to users' own MCP servers: the ones waiting for a code, and the sealed
//! grant of each signed-in server. The Proxy reads neither; the access token is a user secret.

use std::collections::BTreeMap;

use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_schema::tables::{McpOauthPending, UserMcpOauth};
use sea_query::{Expr, ExprTrait as _, OnConflict, Query};

use crate::store::db::{self, DbExt as _, OptionalExtension};
use crate::store::{Store, StoreError};

/// Where a server's sign-in stands, as `mcp-list` and Box starts read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SignIn {
    /// Signed in; the access token expires then, in Unix milliseconds, if it names a time.
    Until(Option<i64>),
    /// The grant could not be refreshed; the user must sign in again.
    Ended,
}

/// A signed-in server whose access token expires.
pub(crate) struct GrantRow {
    /// The user.
    pub(crate) user: UserId,
    /// The server's name.
    pub(crate) name: String,
    /// The sealed grant.
    pub(crate) sealed: Vec<u8>,
    /// When the access token was issued.
    pub(crate) issued: Ts,
    /// When it expires.
    pub(crate) expires: Ts,
}

/// A sign-in waiting for its code.
pub(crate) struct PendingRow {
    /// The server's name.
    pub(crate) name: String,
    /// The sealed sign-in record.
    pub(crate) sealed: Vec<u8>,
    /// When the sign-in stops being finishable.
    pub(crate) expires: Ts,
}

impl Store {
    /// Records a sign-in of `user` to server `name` under `state`, dropping expired ones.
    pub(crate) fn add_mcp_oauth_pending(
        &self,
        state: &str,
        user: UserId,
        (name, sealed): (&str, &[u8]),
        expires: Ts,
    ) -> Result<(), StoreError> {
        self.writer().run(
            Query::delete()
                .from_table(McpOauthPending::Table)
                .and_where(Expr::col(McpOauthPending::ExpiresAt).lte(self.now().as_millis())),
        )?;
        self.writer().run(
            Query::insert()
                .into_table(McpOauthPending::Table)
                .columns([
                    McpOauthPending::State,
                    McpOauthPending::User,
                    McpOauthPending::Name,
                    McpOauthPending::Value,
                    McpOauthPending::ExpiresAt,
                ])
                .values_panic([
                    state.into(),
                    user.get().into(),
                    name.into(),
                    sealed.into(),
                    expires.as_millis().into(),
                ]),
        )?;
        Ok(())
    }

    /// Removes and returns `user`'s sign-in under `state`; another user's stays put.
    pub(crate) fn take_mcp_oauth_pending(
        &self,
        state: &str,
        user: UserId,
    ) -> Result<Option<PendingRow>, StoreError> {
        Ok(self
            .writer()
            .row(
                Query::delete()
                    .from_table(McpOauthPending::Table)
                    .and_where(Expr::col(McpOauthPending::State).eq(state))
                    .and_where(Expr::col(McpOauthPending::User).eq(user.get()))
                    .returning(Query::returning().columns([
                        McpOauthPending::Name,
                        McpOauthPending::Value,
                        McpOauthPending::ExpiresAt,
                    ])),
                |r| {
                    Ok(PendingRow {
                        name: r.get(0)?,
                        sealed: r.get(1)?,
                        expires: Ts::from_millis(r.get(2)?),
                    })
                },
            )
            .optional()?)
    }

    /// Stores `user`'s sealed grant for server `name`, issued at `issued`, replacing any.
    pub(crate) fn set_user_mcp_oauth(
        &self,
        user: UserId,
        (name, sealed): (&str, &[u8]),
        (issued, expires): (Ts, Option<Ts>),
    ) -> Result<(), StoreError> {
        self.writer().run(
            Query::insert()
                .into_table(UserMcpOauth::Table)
                .columns([
                    UserMcpOauth::User,
                    UserMcpOauth::Name,
                    UserMcpOauth::Value,
                    UserMcpOauth::ExpiresAt,
                    UserMcpOauth::UpdatedAt,
                ])
                .values_panic([
                    user.get().into(),
                    name.into(),
                    sealed.into(),
                    expires.map(Ts::as_millis).into(),
                    issued.as_millis().into(),
                ])
                .on_conflict(
                    OnConflict::columns([UserMcpOauth::User, UserMcpOauth::Name])
                        .update_columns([
                            UserMcpOauth::Value,
                            UserMcpOauth::ExpiresAt,
                            UserMcpOauth::UpdatedAt,
                        ])
                        .value(UserMcpOauth::SignedOutAt, Option::<i64>::None)
                        .to_owned(),
                ),
        )?;
        Ok(())
    }

    /// Replaces the grant `row` read, unless it changed since; whether it was replaced.
    pub(crate) fn refresh_user_mcp_oauth(
        &self,
        row: &GrantRow,
        sealed: &[u8],
        (issued, expires): (Ts, Option<Ts>),
    ) -> Result<bool, StoreError> {
        let n = self.writer().run(
            Query::update()
                .table(UserMcpOauth::Table)
                .value(UserMcpOauth::Value, sealed)
                .value(UserMcpOauth::ExpiresAt, expires.map(Ts::as_millis))
                .value(UserMcpOauth::UpdatedAt, issued.as_millis())
                .and_where(Expr::col(UserMcpOauth::User).eq(row.user.get()))
                .and_where(Expr::col(UserMcpOauth::Name).eq(row.name.as_str()))
                .and_where(Expr::col(UserMcpOauth::UpdatedAt).eq(row.issued.as_millis()))
                .and_where(Expr::col(UserMcpOauth::SignedOutAt).is_null()),
        )?;
        Ok(n == 1)
    }

    /// Marks `user`'s server `name` as needing a new sign-in.
    pub(crate) fn end_user_mcp_oauth(&self, user: UserId, name: &str) -> Result<(), StoreError> {
        self.writer().run(
            Query::update()
                .table(UserMcpOauth::Table)
                .value(UserMcpOauth::SignedOutAt, self.now().as_millis())
                .value(UserMcpOauth::ExpiresAt, Option::<i64>::None)
                .and_where(Expr::col(UserMcpOauth::User).eq(user.get()))
                .and_where(Expr::col(UserMcpOauth::Name).eq(name)),
        )?;
        Ok(())
    }

    /// Every signed-in server whose access token names an expiry, for the refresh pass.
    pub(crate) fn expiring_mcp_oauth(&self) -> Result<Vec<GrantRow>, StoreError> {
        let conn = self.reader()?;
        let rows = conn.rows(
            Query::select()
                .columns([
                    UserMcpOauth::User,
                    UserMcpOauth::Name,
                    UserMcpOauth::Value,
                    UserMcpOauth::UpdatedAt,
                    UserMcpOauth::ExpiresAt,
                ])
                .from(UserMcpOauth::Table)
                .and_where(Expr::col(UserMcpOauth::SignedOutAt).is_null())
                .and_where(Expr::col(UserMcpOauth::ExpiresAt).is_not_null()),
            |r| {
                Ok::<_, db::Error>(GrantRow {
                    user: UserId::new(r.get(0)?),
                    name: r.get(1)?,
                    sealed: r.get(2)?,
                    issued: Ts::from_millis(r.get(3)?),
                    expires: Ts::from_millis(r.get(4)?),
                })
            },
        )?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Where each of `user`'s OAuth servers' sign-in stands, by name.
    pub(crate) fn user_mcp_oauth_state(
        &self,
        user: UserId,
    ) -> Result<BTreeMap<String, SignIn>, StoreError> {
        let conn = self.reader()?;
        let rows = conn.rows(
            Query::select()
                .columns([
                    UserMcpOauth::Name,
                    UserMcpOauth::ExpiresAt,
                    UserMcpOauth::SignedOutAt,
                ])
                .from(UserMcpOauth::Table)
                .and_where(Expr::col(UserMcpOauth::User).eq(user.get())),
            |r| {
                let ended = r.get::<_, Option<i64>>(2)?.is_some();
                Ok::<_, db::Error>((
                    r.get(0)?,
                    if ended {
                        SignIn::Ended
                    } else {
                        SignIn::Until(r.get(1)?)
                    },
                ))
            },
        )?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Removes `user`'s grant for server `name`.
    pub(crate) fn delete_user_mcp_oauth(&self, user: UserId, name: &str) -> Result<(), StoreError> {
        self.writer().run(
            Query::delete()
                .from_table(UserMcpOauth::Table)
                .and_where(Expr::col(UserMcpOauth::User).eq(user.get()))
                .and_where(Expr::col(UserMcpOauth::Name).eq(name)),
        )?;
        Ok(())
    }
}
