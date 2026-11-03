//! A node's recent memory reports and pressure state, and the Boxes to resume first.

use std::collections::HashMap;

use moor_core::decide::pressure::{Reading, SUSTAINED_REPORTS};
use moor_core::name::{BoxId, NodeName};

use crate::store::db::{params, DbExt as _, OptionalExtension};
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

impl Store {
    /// `node`'s pressure state.
    pub(crate) fn pressure(&self, node: &NodeName) -> Result<Pressure, StoreError> {
        let conn = self.reader()?;
        let row: Option<(String, Option<i64>, Option<i64>)> = conn
            .query_row(
                "SELECT readings, high_since, suspended_at FROM node_pressure WHERE node = ?1",
                [node.as_str()],
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
        self.writer().execute(
            "INSERT INTO node_pressure (node, readings, high_since, suspended_at) \
             VALUES (?1, ?2, ?3, ?4) ON CONFLICT (node) DO UPDATE SET readings = excluded.readings, \
             high_since = excluded.high_since, suspended_at = excluded.suspended_at",
            params![node.as_str(), readings, p.high_since, p.suspended_at],
        )?;
        Ok(())
    }

    /// Per node name, since when its pressure has been sustained.
    pub(crate) fn pressure_since(&self) -> Result<HashMap<String, i64>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn
            .prepare("SELECT node, high_since FROM node_pressure WHERE high_since IS NOT NULL")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Marks Box `id` to resume first, as suspended at `at`, unix seconds; `None` clears it.
    pub(crate) fn set_resume_first(&self, id: BoxId, at: Option<i64>) -> Result<(), StoreError> {
        self.writer().execute(
            "UPDATE boxes SET resume_first_at = ?1 WHERE id = ?2",
            params![at, id.get()],
        )?;
        Ok(())
    }

    /// When Box `id` was suspended as a last resort, unix seconds; none when it was not.
    pub(crate) fn resume_first(&self, id: BoxId) -> Result<Option<i64>, StoreError> {
        let conn = self.reader()?;
        let at = conn
            .query_row(
                "SELECT resume_first_at FROM boxes WHERE id = ?1",
                [id.get()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(at.flatten())
    }
}
