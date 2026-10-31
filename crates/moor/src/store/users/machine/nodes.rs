//! Every node the scheduler can see: the operator's shared nodes and each enrolled machine that
//! offers a runtime.

use std::collections::{BTreeMap, HashMap};

use moor_core::config::NodeConfig;
use moor_core::decide::capacity::counts;
use moor_core::decide::schedule::{Node, Online, Scope};
use moor_core::domain::boxes::BoxState;
use moor_core::name::NodeName;
use moor_core::user::UserId;

use crate::store::{Store, StoreError};

/// Boxes holding a slot per provider id, counted the way admission counts them.
fn live_per_provider(conn: &rusqlite::Connection) -> Result<HashMap<String, usize>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT provider, driver_state, COUNT(*) FROM boxes \
         WHERE provider IS NOT NULL GROUP BY provider, driver_state",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;
    let mut out = HashMap::new();
    for row in rows {
        let (provider, state, n) = row?;
        if BoxState::parse(&state).is_some_and(counts) {
            *out.entry(provider).or_default() += usize::try_from(n).unwrap_or(0);
        }
    }
    Ok(out)
}

impl Store {
    /// The `shared` nodes and every personal node, with the Boxes holding a slot on each.
    /// `waiting` counts Pending Boxes by the provider id they ask for.
    ///
    /// A personal node is named for its machine; a machine whose name is not a valid node name
    /// is left out.
    pub(crate) fn nodes(
        &self,
        shared: &BTreeMap<String, NodeConfig>,
        waiting: &HashMap<String, usize>,
    ) -> Result<Vec<Node>, StoreError> {
        let conn = self.reader()?;
        let live = live_per_provider(&conn)?;
        let held = |p: &str| live.get(p).copied().unwrap_or(0);
        let pending = |p: &str| waiting.get(p).copied().unwrap_or(0);
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
                    used: held(&cfg.runtime),
                    waiting: pending(&cfg.runtime),
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
            let provider = format!("{runtime}@{name}");
            let (machine_id, owner) = (r.get(0)?, UserId::new(r.get(1)?));
            let max_boxes = r.get::<_, Option<u32>>(8)?.map(|m| m as usize);
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
                used: held(&provider),
                waiting: pending(&provider),
            }))
        })?;
        for row in rows {
            nodes.extend(row?);
        }
        Ok(nodes)
    }
}
