//! Every node the scheduler can see: the operator's shared nodes and each enrolled machine that
//! offers a runtime.

use std::collections::{BTreeMap, HashMap};

use moor_core::config::NodeConfig;
use moor_core::decide::capacity::{counts, holds_memory};
use moor_core::decide::schedule::{Memory, Node, Online, Scope};
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxState;
use moor_core::name::NodeName;
use moor_core::user::UserId;

use crate::store::{Store, StoreError};

/// Boxes per node name.
type Counts = HashMap<String, usize>;
/// MiB per node name.
type Mib = HashMap<String, u64>;

/// Per node: Boxes holding a slot, counted the way admission counts them, Boxes pending, and
/// the MiB its Boxes holding memory were given.
fn per_node(conn: &rusqlite::Connection) -> Result<(Counts, Counts, Mib), StoreError> {
    let mut stmt = conn.prepare(
        "SELECT node, driver_state, state, COUNT(*), \
         COALESCE(SUM(COALESCE(node_mem_mib, mem_mib, 0)), 0) FROM boxes \
         WHERE node IS NOT NULL GROUP BY node, driver_state, state",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, i64>(4)?,
        ))
    })?;
    let (mut live, mut pending, mut held) = (HashMap::new(), HashMap::new(), HashMap::new());
    for row in rows {
        let (node, driver, state, n, mib) = row?;
        let n = usize::try_from(n).unwrap_or(0);
        let driver = driver.and_then(|d| BoxState::parse(&d));
        if driver.is_some_and(holds_memory) {
            *held.entry(node.clone()).or_default() += u64::try_from(mib).unwrap_or(0);
        }
        if driver.is_some_and(counts) {
            *live.entry(node.clone()).or_default() += n;
        }
        if state == BoxLifecycle::Pending.as_str() {
            *pending.entry(node).or_default() += n;
        }
    }
    Ok((live, pending, held))
}

/// A node's memory from what its link reported (`total`, `reserve`, in MiB) and what its Boxes hold.
fn memory_of(total: Option<i64>, reserve: Option<i64>, used_mib: u64) -> Option<Memory> {
    let mib = |m: i64| u64::try_from(m).ok();
    Some(Memory {
        total_mib: total.and_then(mib)?,
        reserve_mib: reserve.and_then(mib),
        used_mib,
    })
}

/// One personal node from a `user_machines_v2` row joined with its link; `load` gives a node's
/// live Boxes, pending Boxes and held MiB by name.
fn personal(
    r: &rusqlite::Row<'_>,
    load: &dyn Fn(&str) -> (usize, usize, u64),
) -> Result<Option<Node>, StoreError> {
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
    let (used, waiting, held_mib) = load(&name);
    let memory = memory_of(r.get(9)?, r.get(10)?, held_mib);
    Ok(NodeName::parse(&name).ok().map(|name| Node {
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
        memory,
    }))
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
        let (live, waiting, mem) = per_node(&conn)?;
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
                    memory: None,
                })
            })
            .collect();
        let mut stmt = conn.prepare(
            "SELECT m.machine_id, m.user, m.name, m.labels, m.pool_provider, m.enrolled_at, \
             l.online, l.since, l.max_boxes, l.mem_mib, l.reserve_mib \
             FROM user_machines_v2 m LEFT JOIN machine_links l USING (machine_id) \
             WHERE m.pool_provider IS NOT NULL ORDER BY m.enrolled_at, m.machine_id",
        )?;
        let rows = stmt.query_and_then([], |r| {
            personal(r, &|n| {
                (held(n), pending(n), mem.get(n).copied().unwrap_or(0))
            })
        })?;
        for row in rows {
            nodes.extend(row?);
        }
        Ok(nodes)
    }
}
