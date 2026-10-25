//! Users, the identities linked to them, one-time link codes, and each user's sealed secrets.

use moor_core::seam::clock::Ts;
use moor_core::user::{Identity, SecretName, UserId};
use moor_core::Secret;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use super::{parsed, Store, StoreError};
use crate::vault::Vault;

pub(super) mod front_leases;
mod mcp;

/// The user `identity` is linked to, on any connection.
fn user_of_in(conn: &Connection, identity: &Identity) -> Result<Option<UserId>, StoreError> {
    let id: Option<i64> = conn
        .query_row(
            "SELECT user FROM identities WHERE identity = ?",
            [identity.as_str()],
            |r| r.get(0),
        )
        .optional()?;
    Ok(id.map(UserId::new))
}

impl Store {
    /// The user `identity` is linked to, creating one for it when it has none; true when this
    /// call created the user.
    pub(crate) fn enroll(&self, identity: &Identity) -> Result<(UserId, bool), StoreError> {
        let now = self.now().as_millis();
        let mut writer = self.writer();
        let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(user) = user_of_in(&tx, identity)? {
            return Ok((user, false));
        }
        tx.execute("INSERT INTO users (created_at) VALUES (?)", [now])?;
        let user = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO identities (identity, user, linked_at) VALUES (?, ?, ?)",
            params![identity.as_str(), user, now],
        )?;
        tx.commit()?;
        drop(writer);
        Ok((UserId::new(user), true))
    }

    /// The user `identity` is linked to, if any.
    pub(crate) fn user_of(&self, identity: &Identity) -> Result<Option<UserId>, StoreError> {
        user_of_in(&self.reader()?, identity)
    }

    /// Every identity linked to `user`, in order.
    pub(crate) fn identities(&self, user: UserId) -> Result<Vec<Identity>, StoreError> {
        let conn = self.reader()?;
        let mut stmt =
            conn.prepare("SELECT identity FROM identities WHERE user = ? ORDER BY identity")?;
        let rows = stmt.query_and_then([user.get()], |r| {
            let raw: String = r.get(0)?;
            parsed("identity", &raw, Identity::parse)
        })?;
        rows.collect()
    }

    /// Records the one-time `code` for `user`, usable until `expires`, and drops expired codes.
    pub(crate) fn add_link_code(
        &self,
        user: UserId,
        code: &str,
        expires: Ts,
    ) -> Result<(), StoreError> {
        let now = self.now().as_millis();
        let mut writer = self.writer();
        let tx = writer.transaction()?;
        tx.execute("DELETE FROM link_codes WHERE expires_at <= ?", [now])?;
        tx.execute(
            "INSERT INTO link_codes (code, user, expires_at) VALUES (?, ?, ?)",
            params![code, user.get(), expires.as_millis()],
        )?;
        tx.commit()?;
        drop(writer);
        Ok(())
    }

    /// Links `identity` to the user that minted `code`, and uses the code up. An unknown, used
    /// or expired code, or an identity that is already linked, is [`StoreError::Invalid`].
    pub(crate) fn redeem_link_code(
        &self,
        code: &str,
        identity: &Identity,
    ) -> Result<UserId, StoreError> {
        let now = self.now().as_millis();
        let mut writer = self.writer();
        let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let found: Option<(i64, i64)> = tx
            .query_row(
                "SELECT user, expires_at FROM link_codes WHERE code = ?",
                [code],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((user, expires)) = found else {
            return Err(StoreError::Invalid("unknown or used link code".into()));
        };
        tx.execute("DELETE FROM link_codes WHERE code = ?", [code])?;
        let refusal = if expires <= now {
            Some("link code expired; mint a new one".to_owned())
        } else {
            user_of_in(&tx, identity)?
                .map(|had| format!("{identity} is already linked to user {}", had.get()))
        };
        if let Some(why) = refusal {
            tx.commit()?;
            return Err(StoreError::Invalid(why));
        }
        tx.execute(
            "INSERT INTO identities (identity, user, linked_at) VALUES (?, ?, ?)",
            params![identity.as_str(), user, now],
        )?;
        tx.commit()?;
        drop(writer);
        Ok(UserId::new(user))
    }

    /// Stores `sealed`, a value the vault encrypted, as `user`'s secret `name`.
    pub(crate) fn set_user_secret(
        &self,
        user: UserId,
        name: &SecretName,
        sealed: &[u8],
    ) -> Result<(), StoreError> {
        let now = self.now().as_millis();
        self.writer().execute(
            "INSERT INTO user_secrets (user, name, value, updated_at) VALUES (?, ?, ?, ?) \
             ON CONFLICT (user, name) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![user.get(), name.as_str(), sealed, now],
        )?;
        Ok(())
    }

    /// Deletes `user`'s secret `name`; true when there was one.
    pub(crate) fn delete_user_secret(
        &self,
        user: UserId,
        name: &SecretName,
    ) -> Result<bool, StoreError> {
        let gone = self.writer().execute(
            "DELETE FROM user_secrets WHERE user = ? AND name = ?",
            params![user.get(), name.as_str()],
        )?;
        Ok(gone == 1)
    }

    /// The names of `user`'s secrets, in order; never their values.
    pub(crate) fn user_secret_names(&self, user: UserId) -> Result<Vec<SecretName>, StoreError> {
        let conn = self.reader()?;
        let mut stmt =
            conn.prepare("SELECT name FROM user_secrets WHERE user = ? ORDER BY name")?;
        let rows = stmt.query_and_then([user.get()], |r| {
            let raw: String = r.get(0)?;
            parsed("secret name", &raw, SecretName::parse)
        })?;
        rows.collect()
    }

    /// `user`'s secret `name`, decrypted with `vault` on every call; nothing is cached, so a
    /// deleted secret is gone for the next caller.
    pub(crate) fn user_secret(
        &self,
        vault: &Vault,
        user: UserId,
        name: &SecretName,
    ) -> Result<Option<Secret>, StoreError> {
        let sealed: Option<Vec<u8>> = self
            .reader()?
            .query_row(
                "SELECT value FROM user_secrets WHERE user = ? AND name = ?",
                params![user.get(), name.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(sealed.map(|s| vault.open(user, name, &s)).transpose()?)
    }
}

#[cfg(test)]
mod tests;
