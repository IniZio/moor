//! Each user's own MCP servers. Their keys are user secrets; these rows never hold one.

use moor_core::config::UserMcp;
use moor_core::user::UserId;
use rusqlite::params;

use crate::store::{Store, StoreError};

impl Store {
    /// Adds `server` to `user`'s list, replacing one of the same name.
    pub(crate) fn set_user_mcp(&self, user: UserId, server: &UserMcp) -> Result<(), StoreError> {
        let now = self.now().as_millis();
        self.writer().execute(
            "INSERT INTO user_mcp_servers (user, name, url, header, secret, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT (user, name) DO UPDATE SET url = excluded.url, \
             header = excluded.header, secret = excluded.secret, updated_at = excluded.updated_at",
            params![
                user.get(),
                server.name,
                server.url,
                server.header,
                server.secret().as_str(),
                now,
                now
            ],
        )?;
        Ok(())
    }

    /// Removes `user`'s server `name`; true when there was one.
    pub(crate) fn delete_user_mcp(&self, user: UserId, name: &str) -> Result<bool, StoreError> {
        let gone = self.writer().execute(
            "DELETE FROM user_mcp_servers WHERE user = ? AND name = ?",
            params![user.get(), name],
        )?;
        Ok(gone == 1)
    }

    /// `user`'s servers by name.
    pub(crate) fn user_mcp(&self, user: UserId) -> Result<Vec<UserMcp>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "SELECT name, url, header FROM user_mcp_servers WHERE user = ? ORDER BY name",
        )?;
        let rows = stmt.query_and_then([user.get()], |r| {
            let (name, url, header): (String, String, String) = (r.get(0)?, r.get(1)?, r.get(2)?);
            UserMcp::parse(&name, &url, Some(&header))
                .map_err(|e| StoreError::Corrupt(format!("user_mcp_servers row {name}: {e}")))
        })?;
        rows.collect()
    }
}
