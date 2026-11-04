//! Each user's own MCP servers. Their keys are user secrets; these rows never hold one.

use moor_core::config::UserMcp;
use moor_core::user::UserId;
use moor_schema::tables::UserMcpServers;
use sea_query::{Expr, ExprTrait as _, OnConflict, Order, Query};

use crate::store::db::DbExt as _;
use crate::store::{Store, StoreError};

impl Store {
    /// Adds `server` to `user`'s list, replacing one of the same name.
    pub(crate) fn set_user_mcp(&self, user: UserId, server: &UserMcp) -> Result<(), StoreError> {
        let now = self.now().as_millis();
        self.writer().run(
            Query::insert()
                .into_table(UserMcpServers::Table)
                .columns([
                    UserMcpServers::User,
                    UserMcpServers::Name,
                    UserMcpServers::Url,
                    UserMcpServers::Header,
                    UserMcpServers::Secret,
                    UserMcpServers::CreatedAt,
                    UserMcpServers::UpdatedAt,
                ])
                .values_panic([
                    user.get().into(),
                    server.name.as_str().into(),
                    server.url.as_str().into(),
                    server.header.as_str().into(),
                    server.secret().as_str().into(),
                    now.into(),
                    now.into(),
                ])
                .on_conflict(
                    OnConflict::columns([UserMcpServers::User, UserMcpServers::Name])
                        .update_columns([
                            UserMcpServers::Url,
                            UserMcpServers::Header,
                            UserMcpServers::Secret,
                            UserMcpServers::UpdatedAt,
                        ])
                        .to_owned(),
                ),
        )?;
        Ok(())
    }

    /// Removes `user`'s server `name`; true when there was one.
    pub(crate) fn delete_user_mcp(&self, user: UserId, name: &str) -> Result<bool, StoreError> {
        let gone = self.writer().run(
            Query::delete()
                .from_table(UserMcpServers::Table)
                .and_where(Expr::col(UserMcpServers::User).eq(user.get()))
                .and_where(Expr::col(UserMcpServers::Name).eq(name)),
        )?;
        Ok(gone == 1)
    }

    /// `user`'s servers by name.
    pub(crate) fn user_mcp(&self, user: UserId) -> Result<Vec<UserMcp>, StoreError> {
        let conn = self.reader()?;
        let rows = conn.rows(
            Query::select()
                .columns([
                    UserMcpServers::Name,
                    UserMcpServers::Url,
                    UserMcpServers::Header,
                ])
                .from(UserMcpServers::Table)
                .and_where(Expr::col(UserMcpServers::User).eq(user.get()))
                .order_by(UserMcpServers::Name, Order::Asc),
            |r| {
                let (name, url, header): (String, String, String) =
                    (r.get(0)?, r.get(1)?, r.get(2)?);
                UserMcp::parse(&name, &url, Some(&header))
                    .map_err(|e| StoreError::Corrupt(format!("user_mcp_servers row {name}: {e}")))
            },
        )?;
        rows.collect()
    }
}
