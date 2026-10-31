//! The node a Box asked for or holds, and the labels it requires of one.

use moor_core::decide::capacity::counts;
use moor_core::decide::schedule::BoxSpec;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxState;
use moor_core::name::NodeName;
use moor_core::BoxId;
use rusqlite::{params, Connection, OptionalExtension};

use super::{Store, StoreError};

/// Boxes holding a slot on `node` as seen on `conn`, counted the way admission counts them.
pub(super) fn live_on_node_in(conn: &Connection, node: &NodeName) -> Result<usize, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT driver_state, COUNT(*) FROM boxes WHERE node = ?1 AND driver_state IS NOT NULL \
         GROUP BY driver_state",
    )?;
    let rows = stmt.query_map([node.as_str()], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })?;
    let mut used = 0;
    for row in rows {
        let (state, n) = row?;
        if BoxState::parse(&state).is_some_and(counts) {
            used += usize::try_from(n).unwrap_or(0);
        }
    }
    Ok(used)
}

impl Store {
    /// What Box `id` asks of a node: the node it named or holds, and its labels.
    pub(crate) fn box_spec(&self, id: BoxId) -> Result<BoxSpec, StoreError> {
        let conn = self.reader()?;
        let found: Option<(Option<String>, String)> = conn
            .query_row(
                "SELECT node, node_labels FROM boxes WHERE id = ?1",
                [id.get()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (node, labels) = found.unwrap_or_else(|| (None, "[]".to_owned()));
        let labels = serde_json::from_str(&labels)
            .map_err(|e| StoreError::Corrupt(format!("box {} node labels: {e}", id.get())))?;
        Ok(BoxSpec {
            labels,
            node: node.and_then(|n| NodeName::parse(&n).ok()),
        })
    }

    /// Records what Box `id` asks of a node.
    pub(crate) fn set_box_spec(&self, id: BoxId, spec: &BoxSpec) -> Result<(), StoreError> {
        let labels = serde_json::to_string(&spec.labels)
            .map_err(|e| StoreError::Corrupt(format!("node labels: {e}")))?;
        self.writer().execute(
            "UPDATE boxes SET node = ?1, node_labels = ?2 WHERE id = ?3",
            params![spec.node.as_ref().map(NodeName::as_str), labels, id.get()],
        )?;
        Ok(())
    }

    /// Pending Boxes waiting for `node`, oldest first.
    pub(crate) fn pending_on(&self, node: &NodeName) -> Result<Vec<BoxId>, StoreError> {
        let conn = self.reader()?;
        let mut stmt =
            conn.prepare("SELECT id FROM boxes WHERE node = ?1 AND state = ?2 ORDER BY id")?;
        let rows = stmt.query_map(
            params![node.as_str(), BoxLifecycle::Pending.as_str()],
            |r| r.get::<_, i64>(0),
        )?;
        rows.map(|r| Ok(BoxId::new(r?))).collect()
    }
}
