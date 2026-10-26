//! OAuth sign-ins to users' own MCP servers: the ones waiting for a code, and the sealed
//! grant of each signed-in server. The Proxy reads neither; the access token is a user secret.

use std::collections::BTreeMap;

use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use rusqlite::{params, OptionalExtension};

use crate::store::{Store, StoreError};

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
        self.writer().execute(
            "DELETE FROM mcp_oauth_pending WHERE expires_at <= ?",
            [self.now().as_millis()],
        )?;
        self.writer().execute(
            "INSERT INTO mcp_oauth_pending (state, user, name, value, expires_at) VALUES (?, ?, ?, ?, ?)",
            params![state, user.get(), name, sealed, expires.as_millis()],
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
            .query_row(
                "DELETE FROM mcp_oauth_pending WHERE state = ? AND user = ? \
                 RETURNING name, value, expires_at",
                params![state, user.get()],
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

    /// Stores `user`'s sealed grant for server `name`, replacing any.
    pub(crate) fn set_user_mcp_oauth(
        &self,
        user: UserId,
        (name, sealed): (&str, &[u8]),
        expires: Option<Ts>,
    ) -> Result<(), StoreError> {
        let now = self.now().as_millis();
        self.writer().execute(
            "INSERT INTO user_mcp_oauth (user, name, value, expires_at, updated_at) VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT (user, name) DO UPDATE SET value = excluded.value, \
             expires_at = excluded.expires_at, updated_at = excluded.updated_at",
            params![user.get(), name, sealed, expires.map(Ts::as_millis), now],
        )?;
        Ok(())
    }

    /// When each of `user`'s signed-in servers' access token expires, by name.
    pub(crate) fn user_mcp_oauth_expiry(
        &self,
        user: UserId,
    ) -> Result<BTreeMap<String, Option<i64>>, StoreError> {
        let conn = self.reader()?;
        let mut stmt =
            conn.prepare("SELECT name, expires_at FROM user_mcp_oauth WHERE user = ?")?;
        let rows = stmt.query_map([user.get()], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Removes `user`'s grant for server `name`.
    pub(crate) fn delete_user_mcp_oauth(&self, user: UserId, name: &str) -> Result<(), StoreError> {
        self.writer().execute(
            "DELETE FROM user_mcp_oauth WHERE user = ? AND name = ?",
            params![user.get(), name],
        )?;
        Ok(())
    }
}
