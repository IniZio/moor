//! Leases of a front's Proxy: each lets requests that present its token spend one user's
//! secrets through that front's Proxy until it is released or expires. Only the hex SHA-256 of
//! the token is stored.

use std::fmt::Write as _;

use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::store::{Store, StoreError};

/// The hex SHA-256 of `token`, which is all the store keeps of it.
fn hash(token: &[u8]) -> String {
    Sha256::digest(token)
        .iter()
        .fold(String::with_capacity(64), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

impl Store {
    /// Records the lease `token` of `front`'s Proxy for `user` until `expires`, and drops
    /// expired ones.
    pub(crate) fn add_front_lease(
        &self,
        token: &[u8],
        front: &str,
        user: UserId,
        expires: Ts,
    ) -> Result<(), StoreError> {
        let conn = self.writer();
        conn.execute(
            "DELETE FROM front_leases WHERE expires_at <= ?",
            [self.now().as_millis()],
        )?;
        conn.execute(
            "INSERT INTO front_leases (token_hash, front, user, expires_at) VALUES (?, ?, ?, ?)",
            params![hash(token), front, user.get(), expires.as_millis()],
        )?;
        drop(conn);
        Ok(())
    }

    /// The user the live lease `token` of `front` acts for; none when there is no such lease,
    /// it is another front's, or it expired.
    pub(crate) fn front_lease_user(
        &self,
        token: &[u8],
        front: &str,
    ) -> Result<Option<UserId>, StoreError> {
        let user: Option<i64> = self
            .reader()?
            .query_row(
                "SELECT user FROM front_leases WHERE token_hash = ? AND front = ? AND expires_at > ?",
                params![hash(token), front, self.now().as_millis()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(user.map(UserId::new))
    }

    /// Ends `user`'s lease `token` of `front`; true when it was there.
    pub(crate) fn delete_front_lease(
        &self,
        token: &[u8],
        front: &str,
        user: UserId,
    ) -> Result<bool, StoreError> {
        let gone = self.writer().execute(
            "DELETE FROM front_leases WHERE token_hash = ? AND front = ? AND user = ?",
            params![hash(token), front, user.get()],
        )?;
        Ok(gone == 1)
    }

    /// Ends every lease; a controller that starts has restarted every front that held one.
    pub(crate) fn purge_front_leases(&self) -> Result<(), StoreError> {
        self.writer().execute("DELETE FROM front_leases", [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
