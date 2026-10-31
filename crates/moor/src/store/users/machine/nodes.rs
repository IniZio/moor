//! Every node the scheduler can see: the operator's shared nodes and each enrolled machine that
//! offers a runtime.

use std::collections::{BTreeMap, HashMap};

use moor_core::config::NodeConfig;
use moor_core::decide::capacity::counts;
use moor_core::decide::schedule::{Node, Online, Scope};
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxState;
use moor_core::name::NodeName;
use moor_core::user::UserId;

use crate::store::{Store, StoreError};

/// Boxes per node name.
type Counts = HashMap<String, usize>;

/// Per node: Boxes holding a slot, counted the way admission counts them, and Boxes pending.
fn per_node(conn: &rusqlite::Connection) -> Result<(Counts, Counts), StoreError> {
    let mut stmt = conn.prepare(
        "SELECT node, driver_state, state, COUNT(*) FROM boxes \
         WHERE node IS NOT NULL GROUP BY node, driver_state, state",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })?;
    let (mut live, mut pending) = (HashMap::new(), HashMap::new());
    for row in rows {
        let (node, driver, state, n) = row?;
        let n = usize::try_from(n).unwrap_or(0);
        if driver.and_then(|d| BoxState::parse(&d)).is_some_and(counts) {
            *live.entry(node.clone()).or_default() += n;
        }
        if state == BoxLifecycle::Pending.as_str() {
            *pending.entry(node).or_default() += n;
        }
    }
    Ok((live, pending))
}

impl Store {
    /// The `shared` nodes and every personal node, with the Boxes holding a slot on or pending
    /// for each.
    ///
    /// A personal node is named for its machine; a machine whose name is not a valid node name
    /// is left out.
    pub(crate) fn nodes(
        &self,
        shared: &BTreeMap<String, NodeConfig>,
    ) -> Result<Vec<Node>, StoreError> {
        let conn = self.reader()?;
        let (live, waiting) = per_node(&conn)?;
        let held = |n: &str| live.get(n).copied().unwrap_or(0);
        let pending = |n: &str| waiting.get(n).copied().unwrap_or(0);
        let mut nodes: Vec<Node> = shared
            .iter()
            .filter_map(|(name, cfg)| {
                Some(Node {
                    name: NodeName::parse(name).ok()?,
                    scope: Scope::Shared,
                    runtime: cfg.runtime.clone(),
                    labels: cfg.labels.clone(),
                    max_boxes: cfg.max_boxes.map(|m| m.get() as usize),
                    online: Online::Online,
                    used: held(name),
                    waiting: pending(name),
                })
            })
            .collect();
        let mut stmt = conn.prepare(
            "SELECT m.machine_id, m.user, m.name, m.labels, m.pool_provider, m.enrolled_at, \
             l.online, l.since, l.max_boxes \
             FROM user_machines_v2 m LEFT JOIN machine_links l USING (machine_id) \
             WHERE m.pool_provider IS NOT NULL ORDER BY m.enrolled_at, m.machine_id",
        )?;
        let rows = stmt.query_and_then([], |r| {
            let name: String = r.get(2)?;
            let labels: String = r.get(3)?;
            let labels = serde_json::from_str(&labels)
                .map_err(|e| StoreError::Corrupt(format!("user_machines_v2 row {name}: {e}")))?;
            let runtime: String = r.get(4)?;
            let enrolled_at: i64 = r.get(5)?;
            let online = match (r.get::<_, Option<bool>>(6)?, r.get::<_, Option<i64>>(7)?) {
                (Some(true), _) => Online::Online,
                (_, since) => Online::Offline {
                    since: since.unwrap_or(enrolled_at) / 1000,
                },
            };
            let (machine_id, owner) = (r.get(0)?, UserId::new(r.get(1)?));
            let max_boxes = r.get::<_, Option<u32>>(8)?.map(|m| m as usize);
            let (used, waiting) = (held(&name), pending(&name));
            Ok::<_, StoreError>(NodeName::parse(&name).ok().map(|name| Node {
                name,
                scope: Scope::Personal {
                    owner,
                    machine_id,
                    enrolled_at: enrolled_at / 1000,
                },
                runtime,
                labels,
                max_boxes,
                online,
                used,
                waiting,
            }))
        })?;
        for row in rows {
            nodes.extend(row?);
        }
        Ok(nodes)
    }
}
