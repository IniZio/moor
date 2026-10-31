//! Which enrolled machines hold a link to the runner, and the keys they prove themselves with.

use moor_core::request::MachineLink;
use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_core::view::user::MachineKey;
use rusqlite::params;

use crate::store::{Store, StoreError};

impl Store {
    /// Every enrolled machine with the key its agent signs with, by id.
    pub(crate) fn machine_keys(&self) -> Result<Vec<MachineKey>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "SELECT machine_id, user, name, pubkey FROM user_machines_v2 ORDER BY machine_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(MachineKey {
                machine_id: r.get(0)?,
                user: UserId::new(r.get(1)?),
                name: r.get(2)?,
                pubkey: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Replaces the stored links with `links`, dropping any for a machine that is not enrolled;
    /// what the store holds afterwards, by machine id.
    pub(crate) fn set_machine_links(
        &self,
        links: &[MachineLink],
    ) -> Result<Vec<MachineLink>, StoreError> {
        let mut conn = self.writer();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM machine_links", [])?;
        for link in links {
            tx.execute(
                "INSERT OR REPLACE INTO machine_links (machine_id, online, since, version, protocol, \
                 max_boxes, cache_gb, pool_features) \
                 SELECT machine_id, ?, ?, ?, ?, ?, ?, ? FROM user_machines_v2 WHERE machine_id = ?",
                params![
                    link.online,
                    link.since.as_millis(),
                    link.version,
                    link.protocol,
                    link.max_boxes,
                    link.cache_gb,
                    serde_json::to_string(&link.features)
                        .map_err(|e| StoreError::Corrupt(format!("link features: {e}")))?,
                    link.machine_id
                ],
            )?;
        }
        let held = {
            let mut stmt = tx.prepare(&format!("SELECT {LINK_COLUMNS} ORDER BY machine_id"))?;
            let rows = stmt.query_map([], link_row)?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        tx.commit()?;
        drop(conn);
        Ok(held)
    }

    /// The stored link of each of `user`'s machines that has one, by machine name.
    pub(crate) fn user_machine_links(
        &self,
        user: UserId,
    ) -> Result<Vec<(String, MachineLink)>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(&format!("SELECT {LINK_COLUMNS} WHERE m.user = ?"))?;
        let rows = stmt.query_map([user.get()], |r| Ok((r.get(8)?, link_row(r)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

/// The columns [`link_row`] reads, then the machine's name once `user_machines_v2 m` is joined.
const LINK_COLUMNS: &str =
    "machine_id, online, since, version, protocol, max_boxes, cache_gb, pool_features, m.name \
     FROM machine_links LEFT JOIN user_machines_v2 m USING (machine_id)";

/// One `machine_links` row.
fn link_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<MachineLink> {
    Ok(MachineLink {
        machine_id: r.get(0)?,
        online: r.get(1)?,
        since: Ts::from_millis(r.get(2)?),
        version: r.get(3)?,
        protocol: r.get(4)?,
        max_boxes: r.get(5)?,
        cache_gb: r.get(6)?,
        features: serde_json::from_str(&r.get::<_, String>(7)?).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(e))
        })?,
    })
}

/// `user`'s enrolled machines with their ids.
pub(super) fn enrolled_in(
    conn: &rusqlite::Connection,
    user: UserId,
) -> Result<Vec<(i64, moor_core::config::UserMachine)>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT machine_id, name, labels, capacity, pool_provider FROM user_machines_v2 WHERE user = ? \
         ORDER BY name",
    )?;
    let rows = stmt.query_and_then([user.get()], |r| {
        let (id, name, labels): (i64, String, String) = (r.get(0)?, r.get(1)?, r.get(2)?);
        let labels = serde_json::from_str(&labels)
            .map_err(|e| StoreError::Corrupt(format!("user_machines_v2 row {name}: {e}")))?;
        let machine = moor_core::config::UserMachine {
            name,
            labels,
            capacity: r.get(3)?,
            pool: r.get(4)?,
        };
        Ok((id, machine))
    })?;
    rows.collect()
}
