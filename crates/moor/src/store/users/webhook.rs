//! Each user's webhooks. The HMAC key and the receiver's header value are sealed by the vault
//! into the row, bound to the user and to `webhook_<id>` or `webhook_<id>_header`, and are not
//! among the user's secrets.

use moor_core::user::{SecretName, UserId};
use moor_core::view::user::{WebhookState, WebhookView};
use moor_core::Secret;
use rusqlite::{params, OptionalExtension as _, TransactionBehavior};

use crate::store::{Store, StoreError};
use crate::vault::Vault;

/// The vault name the key of webhook `id` is sealed under.
pub(crate) fn key_name(id: i64) -> Result<SecretName, StoreError> {
    SecretName::parse(&format!("webhook_{id}")).map_err(StoreError::Invalid)
}

/// The vault name the header value of webhook `id` is sealed under.
pub(crate) fn header_name(id: i64) -> Result<SecretName, StoreError> {
    SecretName::parse(&format!("webhook_{id}_header")).map_err(StoreError::Invalid)
}

/// What the setup page needs of one webhook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WebhookSetupRow {
    /// Where each event is posted.
    pub(crate) url: String,
    /// The header the receiver wants, if any.
    pub(crate) header: Option<String>,
    /// Where it is in its setup.
    pub(crate) state: WebhookState,
    /// The sealed HMAC key.
    pub(crate) sealed_key: Vec<u8>,
}

/// The state stored as `s`.
fn state_of(s: &str) -> Result<WebhookState, StoreError> {
    WebhookState::parse(s).ok_or_else(|| StoreError::Corrupt(format!("user_webhooks state `{s}`")))
}

impl Store {
    /// Adds a webhook for `user`, waiting for setup, with `key` sealed in its row. None, with
    /// nothing stored, when `user` already has `max`.
    pub(crate) fn add_user_webhook(
        &self,
        vault: &Vault,
        user: UserId,
        (url, events, header): (&str, &[String], Option<&str>),
        key: &Secret,
        max: usize,
    ) -> Result<Option<i64>, StoreError> {
        let now = self.now().as_millis();
        let events = serde_json::to_string(events)?;
        let mut writer = self.writer();
        let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let held: i64 = tx.query_row(
            "SELECT COUNT(*) FROM user_webhooks WHERE \"user\" = ?",
            [user.get()],
            |r| r.get(0),
        )?;
        if usize::try_from(held).is_ok_and(|n| n >= max) {
            return Ok(None);
        }
        let id: i64 = tx.query_row(
            "INSERT INTO user_webhooks (\"user\", url, events, header, state, key, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id",
            params![
                user.get(),
                url,
                events,
                header,
                WebhookState::WaitingForSecret.as_str(),
                Vec::<u8>::new(),
                now
            ],
            |r| r.get(0),
        )?;
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
            "SELECT id, url, events, created_at, last_ok_at, failing_since, last_error, header, \
             state FROM user_webhooks WHERE \"user\" = ? ORDER BY id",
        )?;
        let rows = stmt.query_map([user.get()], |r| {
            Ok((
                WebhookView {
                    id: r.get(0)?,
                    url: r.get(1)?,
                    events: Vec::new(),
                    state: WebhookState::Active,
                    header: r.get(7)?,
                    created_at: r.get(3)?,
                    last_ok_at: r.get(4)?,
                    failing_since: r.get(5)?,
                    last_error: r.get(6)?,
                },
                r.get::<_, String>(2)?,
                r.get::<_, String>(8)?,
            ))
        })?;
        rows.map(|row| {
            let (mut view, events, state) = row?;
            view.state = state_of(&state)?;
            view.events = serde_json::from_str(&events)
                .map_err(|e| StoreError::Corrupt(format!("user_webhooks events: {e}")))?;
            Ok(view)
        })
        .collect()
    }

    /// `user`'s webhook `id` as its setup page needs it; None when it is not theirs.
    pub(crate) fn user_webhook_setup(
        &self,
        user: UserId,
        id: i64,
    ) -> Result<Option<WebhookSetupRow>, StoreError> {
        let row = self
            .reader()?
            .query_row(
                "SELECT url, header, state, key FROM user_webhooks WHERE id = ? AND \"user\" = ?",
                params![id, user.get()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, String>(2)?, r.get(3)?)),
            )
            .optional()?;
        row.map(|(url, header, state, sealed_key)| {
            Ok(WebhookSetupRow {
                url,
                header,
                state: state_of(&state)?,
                sealed_key,
            })
        })
        .transpose()
    }

    /// Seals `header_value`, if any, into `user`'s webhook `id` and makes it active. False,
    /// with nothing changed, unless it was theirs and still waiting for setup.
    pub(crate) fn activate_user_webhook(
        &self,
        vault: &Vault,
        user: UserId,
        id: i64,
        header_value: Option<&Secret>,
    ) -> Result<bool, StoreError> {
        let name = header_name(id)?;
        let sealed = header_value
            .map(|value| vault.seal(user, &name, value))
            .transpose()?;
        let changed = self.writer().execute(
            "UPDATE user_webhooks SET header_value = ?, state = ? \
             WHERE id = ? AND \"user\" = ? AND state = ?",
            params![
                sealed,
                WebhookState::Active.as_str(),
                id,
                user.get(),
                WebhookState::WaitingForSecret.as_str()
            ],
        )?;
        Ok(changed == 1)
    }

    /// The sealed header value stored for webhook `id`.
    #[cfg(test)]
    pub(crate) fn webhook_header_blob(&self, id: i64) -> Result<Vec<u8>, StoreError> {
        Ok(self.reader()?.query_row(
            "SELECT header_value FROM user_webhooks WHERE id = ?",
            [id],
            |r| r.get(0),
        )?)
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
             (SELECT id FROM user_webhooks WHERE id = ? AND \"user\" = ?)",
            params![id, user.get()],
        )?;
        let gone = tx.execute(
            "DELETE FROM user_webhooks WHERE id = ? AND \"user\" = ?",
            params![id, user.get()],
        )?;
        tx.commit()?;
        drop(writer);
        Ok(gone == 1)
    }
}
