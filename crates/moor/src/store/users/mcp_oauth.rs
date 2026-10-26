//! OAuth sign-ins to users' own MCP servers: the ones waiting for a code, and the sealed
//! grant of each signed-in server. The Proxy reads neither; the access token is a user secret.

use std::collections::BTreeMap;

use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use rusqlite::{params, OptionalExtension};

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

    /// Stores `user`'s sealed grant for server `name`, issued at `issued`, replacing any.
    pub(crate) fn set_user_mcp_oauth(
        &self,
        user: UserId,
        (name, sealed): (&str, &[u8]),
        (issued, expires): (Ts, Option<Ts>),
    ) -> Result<(), StoreError> {
        self.writer().execute(
            "INSERT INTO user_mcp_oauth (user, name, value, expires_at, updated_at) VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT (user, name) DO UPDATE SET value = excluded.value, \
             expires_at = excluded.expires_at, updated_at = excluded.updated_at, signed_out_at = NULL",
            params![user.get(), name, sealed, expires.map(Ts::as_millis), issued.as_millis()],
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
        let n = self.writer().execute(
            "UPDATE user_mcp_oauth SET value = ?, expires_at = ?, updated_at = ? \
             WHERE user = ? AND name = ? AND updated_at = ? AND signed_out_at IS NULL",
            params![
                sealed,
                expires.map(Ts::as_millis),
                issued.as_millis(),
                row.user.get(),
                row.name,
                row.issued.as_millis()
            ],
        )?;
        Ok(n == 1)
    }

    /// Marks `user`'s server `name` as needing a new sign-in.
    pub(crate) fn end_user_mcp_oauth(&self, user: UserId, name: &str) -> Result<(), StoreError> {
        self.writer().execute(
            "UPDATE user_mcp_oauth SET signed_out_at = ?, expires_at = NULL WHERE user = ? AND name = ?",
            params![self.now().as_millis(), user.get(), name],
        )?;
        Ok(())
    }

    /// Every signed-in server whose access token names an expiry, for the refresh pass.
    pub(crate) fn expiring_mcp_oauth(&self) -> Result<Vec<GrantRow>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "SELECT user, name, value, updated_at, expires_at FROM user_mcp_oauth \
             WHERE signed_out_at IS NULL AND expires_at IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(GrantRow {
                user: UserId::new(r.get(0)?),
                name: r.get(1)?,
                sealed: r.get(2)?,
                issued: Ts::from_millis(r.get(3)?),
                expires: Ts::from_millis(r.get(4)?),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Where each of `user`'s OAuth servers' sign-in stands, by name.
    pub(crate) fn user_mcp_oauth_state(
        &self,
        user: UserId,
    ) -> Result<BTreeMap<String, SignIn>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "SELECT name, expires_at, signed_out_at IS NOT NULL FROM user_mcp_oauth WHERE user = ?",
        )?;
        let rows = stmt.query_map([user.get()], |r| {
            let ended: bool = r.get(2)?;
            Ok((
                r.get(0)?,
                if ended {
                    SignIn::Ended
                } else {
                    SignIn::Until(r.get(1)?)
                },
            ))
        })?;
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
