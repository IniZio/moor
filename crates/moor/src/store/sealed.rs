//! The controller's own sealed values, kept in `meta` as hex: the vault seals them first, so the
//! store holds nothing readable.

use std::fmt::Write as _;

use moor_schema::tables::Meta;
use sea_query::{Expr, ExprTrait as _, OnConflict, Query};

use crate::store::db::{DbExt as _, OptionalExtension};
use crate::store::{Store, StoreError};

impl Store {
    /// The sealed value kept under `k`; none before the first is kept.
    pub(crate) fn sealed(&self, k: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let raw: Option<String> = self
            .reader()?
            .row(
                Query::select()
                    .column(Meta::V)
                    .from(Meta::Table)
                    .and_where(Expr::col(Meta::K).eq(k)),
                |r| r.get(0),
            )
            .optional()?;
        raw.map(|hex| unhex(k, &hex)).transpose()
    }

    /// Keeps `sealed` under `k` unless a value is there; returns the value kept, never a later one.
    pub(crate) fn keep_sealed(&self, k: &str, sealed: &[u8]) -> Result<Vec<u8>, StoreError> {
        let hex = sealed.iter().fold(String::new(), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        });
        self.writer().run(
            Query::insert()
                .into_table(Meta::Table)
                .columns([Meta::K, Meta::V])
                .values_panic([k.into(), hex.into()])
                .on_conflict(OnConflict::column(Meta::K).do_nothing().to_owned()),
        )?;
        self.sealed(k)?
            .ok_or_else(|| StoreError::Corrupt(format!("meta {k} vanished after it was kept")))
    }
}

/// The bytes `hex` spells.
fn unhex(k: &str, hex: &str) -> Result<Vec<u8>, StoreError> {
    let bad = || StoreError::Corrupt(format!("meta {k} is not hex"));
    if !hex.len().is_multiple_of(2) {
        return Err(bad());
    }
    hex.as_bytes()
        .chunks(2)
        .map(|pair| {
            let digits = std::str::from_utf8(pair).map_err(|_| bad())?;
            u8::from_str_radix(digits, 16).map_err(|_| bad())
        })
        .collect()
}
