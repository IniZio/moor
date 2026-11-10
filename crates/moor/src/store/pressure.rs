//! A node's recent memory reports and pressure state, and the Boxes to resume first.

use std::collections::HashMap;

use moor_core::decide::pressure::{Reading, SUSTAINED_REPORTS};
use moor_core::domain::hands::HandsInc;
use moor_core::name::{BoxId, NodeName};
use moor_core::seam::driver::PausePublish;

use moor_schema::tables::{BoxHands, BoxState, NodePressure};
use sea_query::{Expr, ExprTrait as _, OnConflict, Query};

use crate::store::db::{DbExt as _, OptionalExtension};
use crate::store::hands::{fenced, select_current, update_hands};
use crate::store::select::state_update;
use crate::store::{Store, StoreError};

/// A node's pressure state: its recent reports, oldest first, since when pressure has been
/// sustained, and its last suspension, unix seconds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Pressure {
    /// Its last few reports.
    pub(crate) readings: Vec<Reading>,
    /// When sustained pressure began; none without it.
    pub(crate) high_since: Option<i64>,
    /// When it last suspended a Box for pressure.
    pub(crate) suspended_at: Option<i64>,
}

/// What a Box held that was not pushed, checked just before it was suspended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SuspendCheck {
    /// Commits on no remote-tracking ref and not in the base.
    pub(crate) ahead: u32,
    /// Changed or untracked paths.
    pub(crate) dirty: u32,
    /// When it was checked, unix seconds.
    pub(crate) at: i64,
}

impl Store {
    /// `node`'s pressure state.
    pub(crate) fn pressure(&self, node: &NodeName) -> Result<Pressure, StoreError> {
        let conn = self.reader()?;
        let row: Option<(String, Option<i64>, Option<i64>)> = conn
            .row(
                Query::select()
                    .columns([
                        NodePressure::Readings,
                        NodePressure::HighSince,
                        NodePressure::SuspendedAt,
                    ])
                    .from(NodePressure::Table)
                    .and_where(Expr::col(NodePressure::Node).eq(node.as_str())),
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((readings, high_since, suspended_at)) = row else {
            return Ok(Pressure::default());
        };
        let readings = serde_json::from_str(&readings)
            .map_err(|e| StoreError::Corrupt(format!("node {node} readings: {e}")))?;
        Ok(Pressure {
            readings,
            high_since,
            suspended_at,
        })
    }

    /// Stores `p` for `node`, keeping only the reports the pressure decision reads.
    pub(crate) fn set_pressure(&self, node: &NodeName, p: &Pressure) -> Result<(), StoreError> {
        let keep = p
            .readings
            .get(p.readings.len().saturating_sub(SUSTAINED_REPORTS)..)
            .unwrap_or_default();
        let readings = serde_json::to_string(keep)
            .map_err(|e| StoreError::Corrupt(format!("node {node} readings: {e}")))?;
        self.writer().run(
            Query::insert()
                .into_table(NodePressure::Table)
                .columns([
                    NodePressure::Node,
                    NodePressure::Readings,
                    NodePressure::HighSince,
                    NodePressure::SuspendedAt,
                ])
                .values_panic([
                    node.as_str().into(),
                    readings.into(),
                    p.high_since.into(),
                    p.suspended_at.into(),
                ])
                .on_conflict(
                    OnConflict::column(NodePressure::Node)
                        .update_columns([
                            NodePressure::Readings,
                            NodePressure::HighSince,
                            NodePressure::SuspendedAt,
                        ])
                        .to_owned(),
                ),
        )?;
        Ok(())
    }

    /// Per node name, since when its pressure has been sustained.
    pub(crate) fn pressure_since(&self) -> Result<HashMap<String, i64>, StoreError> {
        let conn = self.reader()?;
        let rows = conn.rows(
            Query::select()
                .columns([NodePressure::Node, NodePressure::HighSince])
                .from(NodePressure::Table)
                .and_where(Expr::col(NodePressure::HighSince).is_not_null()),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(rows.collect::<Result<_, crate::store::db::Error>>()?)
    }

    /// Marks Box `id` to resume first, as suspended at `at`, unix seconds; `None` clears it and the
    /// note its agent was owed for that suspension.
    pub(crate) fn set_resume_first(&self, id: BoxId, at: Option<i64>) -> Result<(), StoreError> {
        self.writer()
            .run(state_update(id).value(BoxState::ResumeFirstAt, at))?;
        if at.is_none() {
            self.drop_suspended_note(id)?;
        }
        Ok(())
    }

    /// Stores what Box `id` holds on its hands `inc`, or that nothing is known of it; refused once
    /// those hands are replaced.
    pub(crate) fn set_suspend_check(
        &self,
        (id, inc): (BoxId, HandsInc),
        check: Option<SuspendCheck>,
    ) -> Result<(), StoreError> {
        let written = self.writer().run(
            update_hands(id, inc)
                .value(BoxHands::SuspendAhead, check.map(|c| i64::from(c.ahead)))
                .value(BoxHands::SuspendDirty, check.map(|c| i64::from(c.dirty)))
                .value(BoxHands::SuspendCheckedAt, check.map(|c| c.at)),
        )?;
        fenced(id, inc, written)
    }

    /// What was stored of Box `id` before it was suspended; none when no answer stands.
    pub(crate) fn suspend_check(&self, id: BoxId) -> Result<Option<SuspendCheck>, StoreError> {
        let conn = self.reader()?;
        let row: Option<(Option<i64>, Option<i64>, Option<i64>)> = conn
            .row(
                select_current(
                    id,
                    [
                        BoxHands::SuspendAhead,
                        BoxHands::SuspendDirty,
                        BoxHands::SuspendCheckedAt,
                    ],
                ),
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        Ok(match row {
            Some((Some(ahead), Some(dirty), Some(at))) => Some(SuspendCheck {
                ahead: u32::try_from(ahead).unwrap_or(u32::MAX),
                dirty: u32::try_from(dirty).unwrap_or(u32::MAX),
                at,
            }),
            _ => None,
        })
    }

    /// Stores whether the suspend of Box `id` on its hands `inc` may save its caches; refused once
    /// those hands are replaced.
    pub(crate) fn set_pause_publish(
        &self,
        (id, inc): (BoxId, HandsInc),
        publish: PausePublish,
    ) -> Result<(), StoreError> {
        let allowed = (publish == PausePublish::Allowed).then_some(1_i64);
        let written = self
            .writer()
            .run(update_hands(id, inc).value(BoxHands::SuspendPublish, allowed))?;
        fenced(id, inc, written)
    }

    /// Whether the last suspend of Box `id` may have saved its caches; found stopped, it did not.
    pub(crate) fn pause_publish(&self, id: BoxId) -> Result<PausePublish, StoreError> {
        let conn = self.reader()?;
        let allowed: Option<Option<i64>> = conn
            .row(select_current(id, [BoxHands::SuspendPublish]), |r| r.get(0))
            .optional()?;
        Ok(if allowed.flatten().is_some() {
            PausePublish::Allowed
        } else {
            PausePublish::Denied
        })
    }

    /// When Box `id` was suspended as a last resort, unix seconds; none when it was not.
    pub(crate) fn resume_first(&self, id: BoxId) -> Result<Option<i64>, StoreError> {
        let conn = self.reader()?;
        let at = conn
            .row(
                Query::select()
                    .column(BoxState::ResumeFirstAt)
                    .from(BoxState::Table)
                    .and_where(Expr::col(BoxState::Box).eq(id.get())),
                |r| r.get(0),
            )
            .optional()?;
        Ok(at.flatten())
    }
}
