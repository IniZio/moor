//! Each user's webhooks. The HMAC key is sealed by the vault into the row, bound to the user
//! and to `webhook_<id>`, and is not one of the user's secrets.

use moor_core::user::{SecretName, UserId};
use moor_core::view::user::WebhookView;
use moor_core::Secret;
use rusqlite::{params, TransactionBehavior};

use crate::store::{Store, StoreError};
use crate::vault::Vault;

/// The vault name the key of webhook `id` is sealed under.
pub(crate) fn key_name(id: i64) -> Result<SecretName, StoreError> {
    SecretName::parse(&format!("webhook_{id}")).map_err(StoreError::Invalid)
}

impl Store {
    /// Adds a webhook for `user` with `key` sealed in its row. None, with nothing stored, when
    /// `user` already has `max`.
    pub(crate) fn add_user_webhook(
        &self,
        vault: &Vault,
        user: UserId,
        (url, events): (&str, &[String]),
        key: &Secret,
        max: usize,
    ) -> Result<Option<i64>, StoreError> {
        let now = self.now().as_millis();
        let events = serde_json::to_string(events)?;
        let mut writer = self.writer();
        let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let held: i64 = tx.query_row(
            "SELECT COUNT(*) FROM user_webhooks WHERE user = ?",
            [user.get()],
            |r| r.get(0),
        )?;
        if usize::try_from(held).is_ok_and(|n| n >= max) {
            return Ok(None);
        }
        tx.execute(
            "INSERT INTO user_webhooks (user, url, events, key, created_at) VALUES (?, ?, ?, x'', ?)",
            params![user.get(), url, events, now],
        )?;
        let id = tx.last_insert_rowid();
        let sealed = vault.seal(user, &key_name(id)?, key)?;
        tx.execute(
            "UPDATE user_webhooks SET key = ? WHERE id = ?",
            params![sealed, id],
        )?;
        tx.commit()?;
        drop(writer);
        Ok(Some(id))
    }

    /// `user`'s webhooks, oldest first; never their keys.
    pub(crate) fn user_webhooks(&self, user: UserId) -> Result<Vec<WebhookView>, StoreError> {
        let reader = self.reader()?;
        let mut stmt = reader.prepare(
            "SELECT id, url, events, created_at, last_ok_at, failing_since, last_error \
             FROM user_webhooks WHERE user = ? ORDER BY id",
        )?;
        let rows = stmt.query_map([user.get()], |r| {
            Ok((
                WebhookView {
                    id: r.get(0)?,
                    url: r.get(1)?,
                    events: Vec::new(),
                    created_at: r.get(3)?,
                    last_ok_at: r.get(4)?,
                    failing_since: r.get(5)?,
                    last_error: r.get(6)?,
                },
                r.get::<_, String>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (mut view, events) = row?;
            view.events = serde_json::from_str(&events)
                .map_err(|e| StoreError::Corrupt(format!("user_webhooks events: {e}")))?;
            Ok(view)
        })
        .collect()
    }

    /// The sealed key stored for webhook `id`.
    #[cfg(test)]
    pub(crate) fn webhook_key_blob(&self, id: i64) -> Result<Vec<u8>, StoreError> {
        Ok(self
            .reader()?
            .query_row("SELECT key FROM user_webhooks WHERE id = ?", [id], |r| {
                r.get(0)
            })?)
    }

    /// Deletes `user`'s webhook `id` and its pending deliveries; true when it was theirs.
    pub(crate) fn delete_user_webhook(&self, user: UserId, id: i64) -> Result<bool, StoreError> {
        let mut writer = self.writer();
        let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM webhook_deliveries WHERE webhook = \
             (SELECT id FROM user_webhooks WHERE id = ? AND user = ?)",
            params![id, user.get()],
        )?;
        let gone = tx.execute(
            "DELETE FROM user_webhooks WHERE id = ? AND user = ?",
            params![id, user.get()],
        )?;
        tx.commit()?;
        drop(writer);
        Ok(gone == 1)
    }
}
