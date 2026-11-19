//! The codes machines pair by: each waits for a person to confirm it, then holds the
//! controller's enrollment until the machine's agent collects it once.

use std::time::Duration;

use moor_schema::tables::MachinePairings;
use sea_query::{Expr, ExprTrait as _, OnConflict, Query};

use crate::store::db::{DbExt as _, OptionalExtension};
use crate::store::{Store, StoreError};

/// A code nobody has confirmed.
const WAITING: &str = "waiting";
/// A code a confirm is enrolling now.
const ENROLLING: &str = "enrolling";
/// A code whose enrollment waits for the agent's next poll.
const ENROLLED: &str = "enrolled";
/// A portal-minted token's grant, which no code query may see.
const GRANTED: &str = "granted";
/// What a grant's code starts with, then its owner's identity.
const GRANT_CODE: &str = "grant:";

/// A code to keep.
pub(crate) struct Pairing<'a> {
    /// The code a person confirms.
    pub(crate) code: &'a str,
    /// The hash of the secret the agent polls with.
    pub(crate) device_hash: &'a str,
    /// What the agent asks to be enrolled as, as JSON.
    pub(crate) ask: &'a str,
    /// How long the code lasts.
    pub(crate) ttl: Duration,
}

/// What a poll finds.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Polled {
    /// Not confirmed yet.
    Pending,
    /// Unknown, expired or already collected.
    Expired,
    /// The enrollment as JSON, handed out this once.
    Enrolled(String),
}

impl Store {
    /// Keeps `pairing`, dropping expired codes first: `Some(true)` when kept, `Some(false)`
    /// when another pairing holds its code or device hash, none when `most` codes wait.
    pub(crate) fn start_pairing(
        &self,
        pairing: &Pairing<'_>,
        most: usize,
    ) -> Result<Option<bool>, StoreError> {
        let now = self.now();
        self.writer().run(
            Query::delete()
                .from_table(MachinePairings::Table)
                .and_where(Expr::col(MachinePairings::ExpiresAt).lte(now.as_millis())),
        )?;
        let waiting: i64 = self.writer().row(
            Query::select()
                .expr(Expr::col(MachinePairings::Code).count())
                .from(MachinePairings::Table),
            |r| r.get(0),
        )?;
        if usize::try_from(waiting).unwrap_or(usize::MAX) >= most {
            return Ok(None);
        }
        let kept = self.writer().run(
            Query::insert()
                .into_table(MachinePairings::Table)
                .columns([
                    MachinePairings::Code,
                    MachinePairings::DeviceHash,
                    MachinePairings::Ask,
                    MachinePairings::Stage,
                    MachinePairings::ExpiresAt,
                ])
                .values_panic([
                    pairing.code.into(),
                    pairing.device_hash.into(),
                    pairing.ask.into(),
                    WAITING.into(),
                    now.saturating_add(pairing.ttl).as_millis().into(),
                ])
                .on_conflict(OnConflict::new().do_nothing().to_owned()),
        )?;
        Ok(Some(kept == 1))
    }

    /// What `code` asks for, while it waits for a confirm.
    pub(crate) fn waiting_pairing(&self, code: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .writer()
            .row(
                Query::select()
                    .column(MachinePairings::Ask)
                    .from(MachinePairings::Table)
                    .and_where(Expr::col(MachinePairings::Code).eq(code))
                    .and_where(Expr::col(MachinePairings::Stage).eq(WAITING))
                    .and_where(Expr::col(MachinePairings::ExpiresAt).gt(self.now().as_millis())),
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Moves `code` from waiting to enrolling: what it asks for, or none when it is not waiting.
    /// Only one of two confirms racing for a code gets it.
    pub(crate) fn claim_pairing(&self, code: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .writer()
            .row(
                Query::update()
                    .table(MachinePairings::Table)
                    .value(MachinePairings::Stage, ENROLLING)
                    .and_where(Expr::col(MachinePairings::Code).eq(code))
                    .and_where(Expr::col(MachinePairings::Stage).eq(WAITING))
                    .and_where(Expr::col(MachinePairings::ExpiresAt).gt(self.now().as_millis()))
                    .returning(Query::returning().column(MachinePairings::Ask)),
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Ends the enroll a claim began on `code`: keeps `enrolled` for the agent, or puts the
    /// code back to waiting without one. Whether `code` was enrolling.
    pub(crate) fn settle_pairing(
        &self,
        code: &str,
        enrolled: Option<&str>,
    ) -> Result<bool, StoreError> {
        let stage = if enrolled.is_some() {
            ENROLLED
        } else {
            WAITING
        };
        let changed = self.writer().run(
            Query::update()
                .table(MachinePairings::Table)
                .value(MachinePairings::Stage, stage)
                .value(MachinePairings::Enrolled, enrolled)
                .and_where(Expr::col(MachinePairings::Code).eq(code))
                .and_where(Expr::col(MachinePairings::Stage).eq(ENROLLING)),
        )?;
        Ok(changed == 1)
    }

    /// What the agent polling with the device code that hashes to `device_hash` gets; a
    /// collected enrollment is gone after.
    pub(crate) fn poll_pairing(&self, device_hash: &str) -> Result<Polled, StoreError> {
        let now = self.now().as_millis();
        let collected: Option<String> = self
            .writer()
            .row(
                Query::delete()
                    .from_table(MachinePairings::Table)
                    .and_where(Expr::col(MachinePairings::DeviceHash).eq(device_hash))
                    .and_where(Expr::col(MachinePairings::Stage).eq(ENROLLED))
                    .and_where(Expr::col(MachinePairings::ExpiresAt).gt(now))
                    .returning(Query::returning().column(MachinePairings::Enrolled)),
                |r| r.get(0),
            )
            .optional()?;
        if let Some(enrolled) = collected {
            return Ok(Polled::Enrolled(enrolled));
        }
        let waits = self
            .writer()
            .row(
                Query::select()
                    .column(MachinePairings::Code)
                    .from(MachinePairings::Table)
                    .and_where(Expr::col(MachinePairings::DeviceHash).eq(device_hash))
                    .and_where(Expr::col(MachinePairings::Stage).is_in([WAITING, ENROLLING]))
                    .and_where(Expr::col(MachinePairings::ExpiresAt).gt(now)),
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .is_some();
        Ok(if waits {
            Polled::Pending
        } else {
            Polled::Expired
        })
    }
}

impl Store {
    /// Keeps a grant of `owner` (as JSON) for the token that hashes to `token_hash`, replacing
    /// the one that owner held: whether it was kept, none when `most` other rows wait.
    pub(crate) fn grant_pairing(
        &self,
        (identity, owner): (&str, &str),
        token_hash: &str,
        ttl: Duration,
        most: usize,
    ) -> Result<bool, StoreError> {
        let now = self.now();
        let code = format!("{GRANT_CODE}{identity}");
        self.writer().run(
            Query::delete()
                .from_table(MachinePairings::Table)
                .and_where(Expr::col(MachinePairings::ExpiresAt).lte(now.as_millis())),
        )?;
        let others: i64 = self.writer().row(
            Query::select()
                .expr(Expr::col(MachinePairings::Code).count())
                .from(MachinePairings::Table)
                .and_where(Expr::col(MachinePairings::Code).ne(code.as_str())),
            |r| r.get(0),
        )?;
        if usize::try_from(others).unwrap_or(usize::MAX) >= most {
            return Ok(false);
        }
        self.writer().run(
            Query::insert()
                .into_table(MachinePairings::Table)
                .columns([
                    MachinePairings::Code,
                    MachinePairings::DeviceHash,
                    MachinePairings::Ask,
                    MachinePairings::Stage,
                    MachinePairings::ExpiresAt,
                ])
                .values_panic([
                    code.into(),
                    token_hash.into(),
                    owner.into(),
                    GRANTED.into(),
                    now.saturating_add(ttl).as_millis().into(),
                ])
                .on_conflict(
                    OnConflict::column(MachinePairings::Code)
                        .update_columns([
                            MachinePairings::DeviceHash,
                            MachinePairings::Ask,
                            MachinePairings::Stage,
                            MachinePairings::ExpiresAt,
                        ])
                        .to_owned(),
                ),
        )?;
        Ok(true)
    }

    /// Takes the unexpired grant whose token hashes to `token_hash`, once: its owner as JSON.
    pub(crate) fn redeem_pairing(&self, token_hash: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .writer()
            .row(
                Query::delete()
                    .from_table(MachinePairings::Table)
                    .and_where(Expr::col(MachinePairings::DeviceHash).eq(token_hash))
                    .and_where(Expr::col(MachinePairings::Stage).eq(GRANTED))
                    .and_where(Expr::col(MachinePairings::ExpiresAt).gt(self.now().as_millis()))
                    .returning(Query::returning().column(MachinePairings::Ask)),
                |r| r.get(0),
            )
            .optional()?)
    }
}

#[cfg(test)]
mod tests;
