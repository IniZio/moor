//! Leases of a front's Proxy: each lets requests that present its token spend one user's
//! secrets through that front's Proxy until it is released or expires. Only the hex SHA-256 of
//! the token is stored.

use std::fmt::Write as _;

use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_schema::tables::FrontLeases;
use sea_query::{Expr, ExprTrait as _, Query};
use sha2::{Digest, Sha256};

use crate::store::db::{DbExt as _, OptionalExtension};
use crate::store::{Store, StoreError};

/// The hex SHA-256 of `token`, which is all the store keeps of it.
pub(crate) fn hash(token: &[u8]) -> String {
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
        conn.run(
            Query::delete()
                .from_table(FrontLeases::Table)
                .and_where(Expr::col(FrontLeases::ExpiresAt).lte(self.now().as_millis())),
        )?;
        conn.run(
            Query::insert()
                .into_table(FrontLeases::Table)
                .columns([
                    FrontLeases::TokenHash,
                    FrontLeases::Front,
                    FrontLeases::User,
                    FrontLeases::ExpiresAt,
                ])
                .values_panic([
                    hash(token).into(),
                    front.into(),
                    user.get().into(),
                    expires.as_millis().into(),
                ]),
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
            .row(
                Query::select()
                    .column(FrontLeases::User)
                    .from(FrontLeases::Table)
                    .and_where(Expr::col(FrontLeases::TokenHash).eq(hash(token)))
                    .and_where(Expr::col(FrontLeases::Front).eq(front))
                    .and_where(Expr::col(FrontLeases::ExpiresAt).gt(self.now().as_millis())),
                |r| r.get(0),
            )
            .optional()?;
        Ok(user.map(UserId::new))
    }

    /// The front and user of the live lease whose token hashes to `token_hash`.
    pub(crate) fn front_lease_of(
        &self,
        token_hash: &str,
    ) -> Result<Option<(String, UserId)>, StoreError> {
        let row: Option<(String, i64)> = self
            .reader()?
            .row(
                Query::select()
                    .columns([FrontLeases::Front, FrontLeases::User])
                    .from(FrontLeases::Table)
                    .and_where(Expr::col(FrontLeases::TokenHash).eq(token_hash))
                    .and_where(Expr::col(FrontLeases::ExpiresAt).gt(self.now().as_millis())),
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row.map(|(front, user)| (front, UserId::new(user))))
    }

    /// Whether `user` holds a live lease of `front`'s Proxy.
    pub(crate) fn front_lease_held(&self, user: UserId, front: &str) -> Result<bool, StoreError> {
        let found: Option<i64> = self
            .reader()?
            .row(
                Query::select()
                    .column(FrontLeases::User)
                    .from(FrontLeases::Table)
                    .and_where(Expr::col(FrontLeases::User).eq(user.get()))
                    .and_where(Expr::col(FrontLeases::Front).eq(front))
                    .and_where(Expr::col(FrontLeases::ExpiresAt).gt(self.now().as_millis()))
                    .limit(1),
                |r| r.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// Ends `user`'s lease `token` of `front`; true when it was there.
    pub(crate) fn delete_front_lease(
        &self,
        token: &[u8],
        front: &str,
        user: UserId,
    ) -> Result<bool, StoreError> {
        let gone = self.writer().run(
            Query::delete()
                .from_table(FrontLeases::Table)
                .and_where(Expr::col(FrontLeases::TokenHash).eq(hash(token)))
                .and_where(Expr::col(FrontLeases::Front).eq(front))
                .and_where(Expr::col(FrontLeases::User).eq(user.get())),
        )?;
        Ok(gone == 1)
    }

    /// Ends every lease of a front outside `remote`; a controller that starts has restarted
    /// every such front, while a remote front runs on and its turn still holds its lease.
    pub(crate) fn purge_front_leases<'a>(
        &self,
        remote: impl IntoIterator<Item = &'a String>,
    ) -> Result<(), StoreError> {
        let remote: Vec<&str> = remote.into_iter().map(String::as_str).collect();
        self.writer().run(
            Query::delete()
                .from_table(FrontLeases::Table)
                .and_where(Expr::col(FrontLeases::Front).is_not_in(remote)),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
