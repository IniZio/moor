//! Users, the identities linked to them, and each user's sealed secrets.

use moor_core::user::{Email, Identity, SecretName, UserId};
use moor_core::Secret;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use super::{parsed, Store, StoreError};
use crate::vault::Vault;

pub(super) mod front_leases;
pub(super) mod machine;
mod mcp;
pub(super) mod mcp_oauth;

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
    ///
    /// With a verified `email`, an unknown identity joins the user that has the email, and a
    /// new user is created with it; a known identity's user gains the email when it has none
    /// and no other user holds it. No two users ever share an email.
    pub(crate) fn enroll(
        &self,
        identity: &Identity,
        email: Option<&Email>,
    ) -> Result<(UserId, bool), StoreError> {
        let now = self.now().as_millis();
        let email = email.map(Email::as_str);
        let mut writer = self.writer();
        let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(user) = user_of_in(&tx, identity)? {
            if let Some(email) = email {
                tx.execute(
                    "UPDATE users SET email = ?1 WHERE id = ?2 AND email IS NULL \
                     AND NOT EXISTS (SELECT 1 FROM users WHERE email = ?1)",
                    params![email, user.get()],
                )?;
            }
            tx.commit()?;
            return Ok((user, false));
        }
        let holder: Option<i64> = match email {
            Some(email) => tx
                .query_row("SELECT id FROM users WHERE email = ?", [email], |r| {
                    r.get(0)
                })
                .optional()?,
            None => None,
        };
        let (user, created) = if let Some(user) = holder {
            (user, false)
        } else {
            let user = tx.query_row(
                "INSERT INTO users (created_at, email) VALUES (?, ?) RETURNING id",
                params![now, email],
                |r| r.get(0),
            )?;
            (user, true)
        };
        tx.execute(
            "INSERT INTO identities (identity, user, linked_at) VALUES (?, ?, ?)",
            params![identity.as_str(), user, now],
        )?;
        tx.commit()?;
        drop(writer);
        Ok((UserId::new(user), created))
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
