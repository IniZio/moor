//! The wanted action and the effect in flight, as a Box read row holds them.

use moor_core::domain::decision::{Effect, Wanted, WantedKind};
use moor_core::RunId;
use moor_schema::tables::BoxState;
use sea_query::{Expr, ExprTrait as _, Order};

use super::db::Row;
use super::reads::{query_boxes, SHUT};
use super::select::state_at;
use super::{BoxRow, Store, StoreError};

impl Store {
    /// The open Boxes a process that stopped left something in flight on: an effect, or a close
    /// it wanted. Only a close outlives a restart; a wanted sleep or suspend is remade from what
    /// holds then.
    pub(crate) fn in_flight_boxes(&self) -> Result<Vec<BoxRow>, StoreError> {
        let col = |c| Expr::col((BoxState::Table, c));
        let in_flight = col(BoxState::State).is_not_in(SHUT).and(
            col(BoxState::Effect)
                .is_not_null()
                .or(col(BoxState::Wanted).eq(WantedKind::Close.as_str())),
        );
        query_boxes(&self.reader()?, in_flight, Order::Asc, None)
    }

    /// The open Boxes that want something of `kinds`, by id.
    pub(crate) fn wanting(&self, kinds: &[WantedKind]) -> Result<Vec<BoxRow>, StoreError> {
        let col = |c| Expr::col((BoxState::Table, c));
        let names = kinds.iter().map(|k| k.as_str());
        let wanting = col(BoxState::State)
            .is_not_in(SHUT)
            .and(col(BoxState::Wanted).is_in(names));
        query_boxes(&self.reader()?, wanting, Order::Asc, None)
    }

    /// The open Boxes that want a close and have no effect in flight, by id.
    pub(crate) fn wanting_close(&self) -> Result<Vec<BoxRow>, StoreError> {
        let rows = self.wanting(&[WantedKind::Close])?;
        Ok(rows.into_iter().filter(|r| r.effect.is_none()).collect())
    }
}

/// The wanted action a Box read row holds.
pub(super) fn wanted_from(row: &Row<'_>) -> Result<Option<Wanted>, StoreError> {
    let Some(kind) = row.get::<_, Option<String>>(const { state_at(BoxState::Wanted) })? else {
        return Ok(None);
    };
    let force = row
        .get::<_, Option<i64>>(const { state_at(BoxState::WantedForce) })?
        .is_some_and(|f| f != 0);
    let reason = row.get(const { state_at(BoxState::WantedReason) })?;
    Wanted::from_columns(&kind, force, reason)
        .map(Some)
        .ok_or_else(|| StoreError::Corrupt(format!("wanted `{kind}`")))
}

/// The effect in flight a Box read row holds.
pub(super) fn effect_from(row: &Row<'_>) -> Result<Option<Effect>, StoreError> {
    let Some(kind) = row.get::<_, Option<String>>(const { state_at(BoxState::Effect) })? else {
        return Ok(None);
    };
    let run = row
        .get::<_, Option<i64>>(const { state_at(BoxState::EffectRun) })?
        .map(RunId::new);
    Effect::from_columns(&kind, run)
        .map(Some)
        .ok_or_else(|| StoreError::Corrupt(format!("effect `{kind}`")))
}
