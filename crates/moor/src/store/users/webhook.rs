//! Each user's webhooks. The HMAC key and the receiver's header value are sealed by the vault
//! into the row, bound to the user and to `webhook_<id>` or `webhook_<id>_header`, and are not
//! among the user's secrets.

use moor_core::user::{SecretName, UserId};
use moor_core::view::user::{WebhookState, WebhookView};
use moor_core::Secret;
use moor_schema::tables::{UserWebhooks, WebhookDeliveries};
use sea_query::{Expr, ExprTrait as _, Order, Query};

use crate::store::db::{self, DbExt as _, OptionalExtension as _};
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
        let tx = writer.immediate()?;
        let held: i64 = tx.row(
            Query::select()
                .expr(Expr::col(UserWebhooks::Id).count())
                .from(UserWebhooks::Table)
                .and_where(Expr::col(UserWebhooks::User).eq(user.get())),
            |r| r.get(0),
        )?;
        if usize::try_from(held).is_ok_and(|n| n >= max) {
            return Ok(None);
        }
        let id: i64 = tx.row(
            Query::insert()
                .into_table(UserWebhooks::Table)
                .columns([
                    UserWebhooks::User,
                    UserWebhooks::Url,
                    UserWebhooks::Events,
                    UserWebhooks::Header,
                    UserWebhooks::State,
                    UserWebhooks::Key,
                    UserWebhooks::CreatedAt,
                ])
                .values_panic([
                    user.get().into(),
                    url.into(),
                    events.into(),
                    header.into(),
                    WebhookState::WaitingForSecret.as_str().into(),
                    Vec::<u8>::new().into(),
                    now.into(),
                ])
                .returning_col(UserWebhooks::Id),
            |r| r.get(0),
        )?;
        let sealed = vault.seal(user, &key_name(id)?, key)?;
        tx.run(
            Query::update()
                .table(UserWebhooks::Table)
                .value(UserWebhooks::Key, sealed)
                .and_where(Expr::col(UserWebhooks::Id).eq(id)),
        )?;
        tx.commit()?;
        drop(writer);
        Ok(Some(id))
    }

    /// `user`'s webhooks, oldest first; never their keys.
    pub(crate) fn user_webhooks(&self, user: UserId) -> Result<Vec<WebhookView>, StoreError> {
        let reader = self.reader()?;
        let rows = reader.rows(
            Query::select()
                .columns([
                    UserWebhooks::Id,
                    UserWebhooks::Url,
                    UserWebhooks::Events,
                    UserWebhooks::CreatedAt,
                    UserWebhooks::LastOkAt,
                    UserWebhooks::FailingSince,
                    UserWebhooks::LastError,
                    UserWebhooks::Header,
                    UserWebhooks::State,
                ])
                .from(UserWebhooks::Table)
                .and_where(Expr::col(UserWebhooks::User).eq(user.get()))
                .order_by(UserWebhooks::Id, Order::Asc),
            |r| {
                Ok::<_, db::Error>((
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
            },
        )?;
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
            .row(
                Query::select()
                    .columns([
                        UserWebhooks::Url,
                        UserWebhooks::Header,
                        UserWebhooks::State,
                        UserWebhooks::Key,
                    ])
                    .from(UserWebhooks::Table)
                    .and_where(Expr::col(UserWebhooks::Id).eq(id))
                    .and_where(Expr::col(UserWebhooks::User).eq(user.get())),
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
        let changed = self.writer().run(
            Query::update()
                .table(UserWebhooks::Table)
                .value(UserWebhooks::HeaderValue, sealed)
                .value(UserWebhooks::State, WebhookState::Active.as_str())
                .and_where(Expr::col(UserWebhooks::Id).eq(id))
                .and_where(Expr::col(UserWebhooks::User).eq(user.get()))
                .and_where(
                    Expr::col(UserWebhooks::State).eq(WebhookState::WaitingForSecret.as_str()),
                ),
        )?;
        Ok(changed == 1)
    }

    /// The sealed header value stored for webhook `id`.
    #[cfg(test)]
    pub(crate) fn webhook_header_blob(&self, id: i64) -> Result<Vec<u8>, StoreError> {
        Ok(self.reader()?.row(
            Query::select()
                .column(UserWebhooks::HeaderValue)
                .from(UserWebhooks::Table)
                .and_where(Expr::col(UserWebhooks::Id).eq(id)),
            |r| r.get(0),
        )?)
    }

    /// The sealed key stored for webhook `id`.
    #[cfg(test)]
    pub(crate) fn webhook_key_blob(&self, id: i64) -> Result<Vec<u8>, StoreError> {
        Ok(self.reader()?.row(
            Query::select()
                .column(UserWebhooks::Key)
                .from(UserWebhooks::Table)
                .and_where(Expr::col(UserWebhooks::Id).eq(id)),
            |r| r.get(0),
        )?)
    }

    /// Deletes `user`'s webhook `id` and its pending deliveries; true when it was theirs.
    pub(crate) fn delete_user_webhook(&self, user: UserId, id: i64) -> Result<bool, StoreError> {
        let mut writer = self.writer();
        let tx = writer.immediate()?;
        let theirs = Query::select()
            .column(UserWebhooks::Id)
            .from(UserWebhooks::Table)
            .and_where(Expr::col(UserWebhooks::Id).eq(id))
            .and_where(Expr::col(UserWebhooks::User).eq(user.get()))
            .take();
        tx.run(
            Query::delete()
                .from_table(WebhookDeliveries::Table)
                .and_where(Expr::col(WebhookDeliveries::Webhook).eq(db::sub(theirs))),
        )?;
        let gone = tx.run(
            Query::delete()
                .from_table(UserWebhooks::Table)
                .and_where(Expr::col(UserWebhooks::Id).eq(id))
                .and_where(Expr::col(UserWebhooks::User).eq(user.get())),
        )?;
        tx.commit()?;
        drop(writer);
        Ok(gone == 1)
    }
}
