//! The node a Box asked for or holds, and the labels it requires of one.

use moor_core::decide::capacity::counts;
use moor_core::decide::schedule::BoxSpec;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxState;
use moor_core::name::NodeName;
use moor_core::BoxId;
use rusqlite::{params, Connection, OptionalExtension};

use super::{Store, StoreError};

/// Boxes holding a slot on `node` as seen on `conn`, counted the way admission counts them,
/// and Boxes setting up there.
pub(super) fn live_on_node_in(
    conn: &Connection,
    node: &NodeName,
) -> Result<(usize, usize), StoreError> {
    let mut stmt = conn.prepare(
        "SELECT driver_state, state, COUNT(*) FROM boxes WHERE node = ?1 GROUP BY driver_state, state",
    )?;
    let rows = stmt.query_map([node.as_str()], |r| {
        Ok((
            r.get::<_, Option<String>>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;
    let (mut used, mut starting) = (0, 0);
    for row in rows {
        let (driver, state, n) = row?;
        let n = usize::try_from(n).unwrap_or(0);
        if driver.and_then(|d| BoxState::parse(&d)).is_some_and(counts) {
            used += n;
        }
        if state == BoxLifecycle::SettingUp.as_str() {
            starting += n;
        }
    }
    Ok((used, starting))
}

impl Store {
    /// The Boxes whose lifecycle is Ready on `node`.
    pub(crate) fn ready_on(&self, node: &NodeName) -> Result<Vec<BoxId>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare("SELECT id FROM boxes WHERE node = ?1 AND state = ?2")?;
        let rows = stmt.query_map(params![node.as_str(), BoxLifecycle::Ready.as_str()], |r| {
            Ok(BoxId::new(r.get(0)?))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// What Box `id` asks of a node: the node it named or holds, and its labels.
    pub(crate) fn box_spec(&self, id: BoxId) -> Result<BoxSpec, StoreError> {
        let conn = self.reader()?;
        let found: Option<(Option<String>, String, Option<i64>)> = conn
            .query_row(
                "SELECT node, node_labels, node_mem_mib FROM boxes WHERE id = ?1",
                [id.get()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (node, labels, mem) = found.unwrap_or_else(|| (None, "[]".to_owned(), None));
        let labels = serde_json::from_str(&labels)
            .map_err(|e| StoreError::Corrupt(format!("box {} node labels: {e}", id.get())))?;
        Ok(BoxSpec {
            labels,
            node: node.and_then(|n| NodeName::parse(&n).ok()),
            mem_mib: mem.and_then(|m| u64::try_from(m).ok()).unwrap_or(0),
        })
    }

    /// Records what Box `id` asks of a node.
    pub(crate) fn set_box_spec(&self, id: BoxId, spec: &BoxSpec) -> Result<(), StoreError> {
        let labels = serde_json::to_string(&spec.labels)
            .map_err(|e| StoreError::Corrupt(format!("node labels: {e}")))?;
        self.writer().execute(
            "UPDATE boxes SET node = ?1, node_labels = ?2, node_mem_mib = ?3 WHERE id = ?4",
            params![
                spec.node.as_ref().map(NodeName::as_str),
                labels,
                i64::try_from(spec.mem_mib).unwrap_or(i64::MAX),
                id.get()
            ],
        )?;
        Ok(())
    }

    /// Puts Box `id` on `node` when it is on none; returns whether it was.
    pub(crate) fn place_unplaced(&self, id: BoxId, node: &NodeName) -> Result<bool, StoreError> {
        let n = self.writer().execute(
            "UPDATE boxes SET node = ?1 WHERE id = ?2 AND node IS NULL",
            params![node.as_str(), id.get()],
        )?;
        Ok(n > 0)
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
