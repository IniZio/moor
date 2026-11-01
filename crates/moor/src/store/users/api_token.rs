//! Each user's API tokens. Only a token's SHA-256 is stored, with its last 4 characters for
//! display, so the table cannot give a token back.

use moor_core::user::{Email, Identity, UserId};
use moor_core::view::user::ApiTokenView;
use rusqlite::{params, OptionalExtension as _, TransactionBehavior};

use crate::store::{Store, StoreError};

/// How long, in milliseconds, a token's `last_used_at` stays before a use moves it.
const LAST_USED_STEP_MS: i64 = 60_000;

/// A new token's row.
pub(crate) struct NewApiToken<'a> {
    /// The name its person gave it.
    pub(crate) name: &'a str,
    /// What it may do.
    pub(crate) scopes: &'a [String],
    /// The SHA-256 of its text.
    pub(crate) hash: &'a [u8],
    /// Its last 4 characters.
    pub(crate) hint: &'a str,
}

/// Why a token was not stored.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NotAdded {
    /// The user holds as many tokens as they may.
    Full,
    /// The user has a token of that name.
    NameTaken,
}

/// Whom a token speaks for, as `api-token-resolve` answers.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ResolvedToken {
    /// The owner's identity in the namespace asked about.
    pub(crate) identity: Identity,
    /// The owner's verified email.
    pub(crate) email: Option<Email>,
    /// What the token may do.
    pub(crate) scopes: Vec<String>,
}

/// The scopes stored as `json`.
fn scopes_of(json: &str) -> Result<Vec<String>, StoreError> {
    serde_json::from_str(json).map_err(|e| StoreError::Corrupt(format!("api_tokens scopes: {e}")))
}

impl Store {
    /// Stores `token` for `user`; its id, or why nothing was stored.
    pub(crate) fn add_api_token(
        &self,
        user: UserId,
        token: &NewApiToken<'_>,
        max: usize,
    ) -> Result<Result<i64, NotAdded>, StoreError> {
        let now = self.now().as_millis();
        let scopes = serde_json::to_string(token.scopes)?;
        let mut writer = self.writer();
        let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (held, name_used): (i64, bool) = tx.query_row(
            "SELECT COUNT(*), COALESCE(SUM(name = ?2), 0) > 0 FROM api_tokens WHERE user = ?1",
            params![user.get(), token.name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if name_used {
            return Ok(Err(NotAdded::NameTaken));
        }
        if usize::try_from(held).is_ok_and(|n| n >= max) {
            return Ok(Err(NotAdded::Full));
        }
        tx.execute(
            "INSERT INTO api_tokens (user, name, scopes, hash, hint, created_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
            params![user.get(), token.name, scopes, token.hash, token.hint, now],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        drop(writer);
        Ok(Ok(id))
    }

    /// `user`'s tokens, oldest first.
    pub(crate) fn api_tokens(&self, user: UserId) -> Result<Vec<ApiTokenView>, StoreError> {
        let reader = self.reader()?;
        let mut stmt = reader.prepare(
            "SELECT id, name, scopes, hint, created_at, last_used_at FROM api_tokens \
             WHERE user = ? ORDER BY id",
        )?;
        let rows = stmt.query_map([user.get()], |r| {
            Ok((
                ApiTokenView {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    scopes: Vec::new(),
                    hint: r.get(3)?,
                    created_at: r.get(4)?,
                    last_used_at: r.get(5)?,
                },
                r.get::<_, String>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (mut view, scopes) = row?;
            view.scopes = scopes_of(&scopes)?;
            Ok(view)
        })
        .collect()
    }

    /// Deletes `user`'s token `id`; true when it was theirs.
    pub(crate) fn delete_api_token(&self, user: UserId, id: i64) -> Result<bool, StoreError> {
        let gone = self.writer().execute(
            "DELETE FROM api_tokens WHERE id = ? AND user = ?",
            params![id, user.get()],
        )?;
        Ok(gone == 1)
    }

    /// The owner, in namespace `ns`, and scopes of the token whose SHA-256 is `hash`; None when
    /// no token has it or its owner has no `ns` identity. Moves its `last_used_at` at most once
    /// a minute.
    pub(crate) fn resolve_api_token(
        &self,
        hash: &[u8],
        ns: &str,
    ) -> Result<Option<ResolvedToken>, StoreError> {
        let row = self
            .reader()?
            .query_row(
                "SELECT t.id, t.user, t.scopes, u.email FROM api_tokens t \
                 JOIN users u ON u.id = t.user WHERE t.hash = ?",
                [hash],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        UserId::new(r.get(1)?),
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((id, user, scopes, email)) = row else {
            return Ok(None);
        };
        let Some(identity) = self
            .identities(user)?
            .into_iter()
            .find(|i| i.namespace() == ns)
        else {
            return Ok(None);
        };
        let now = self.now().as_millis();
        self.writer().execute(
            "UPDATE api_tokens SET last_used_at = ?1 WHERE id = ?2 \
             AND (last_used_at IS NULL OR last_used_at <= ?1 - ?3)",
            params![now, id, LAST_USED_STEP_MS],
        )?;
        let email = email
            .map(|e| Email::parse(&e).map_err(StoreError::Corrupt))
            .transpose()?;
        Ok(Some(ResolvedToken {
            identity,
            email,
            scopes: scopes_of(&scopes)?,
        }))
    }
}
