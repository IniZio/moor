//! Which enrolled machines hold a link to the runner, and the keys they prove themselves with.

use std::collections::HashMap;

use moor_core::request::{MachineLink, MemoryReport};
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
        // A relinked agent has not reported yet, but its node still reports memory.
        let reported: HashMap<i64, MemoryReport> = {
            let mut stmt = tx.prepare(&format!(
                "SELECT {LINK_COLUMNS} WHERE mem_available_mib IS NOT NULL"
            ))?;
            let rows = stmt.query_map([], link_row)?;
            rows.filter_map(|l| l.map(|l| l.memory.map(|m| (l.machine_id, m))).transpose())
                .collect::<Result<_, _>>()?
        };
        tx.execute("DELETE FROM machine_links", [])?;
        for link in links {
            let memory = link
                .memory
                .or_else(|| reported.get(&link.machine_id).copied());
            tx.execute(
                "INSERT OR REPLACE INTO machine_links (machine_id, online, since, version, protocol, \
                 max_boxes, cache_gb, pool_features, mem_mib, cpus, reserve_mib, mem_available_mib, \
                 psi_centi, reported_at, psi_full_centi, disk_free_mib, disk_total_mib) \
                 SELECT machine_id, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ? FROM user_machines_v2 \
                 WHERE machine_id = ?",
                params![
                    link.online,
                    link.since.as_millis(),
                    link.version,
                    link.protocol,
                    link.max_boxes,
                    link.cache_gb,
                    serde_json::to_string(&link.features)
                        .map_err(|e| StoreError::Corrupt(format!("link features: {e}")))?,
                    link.mem_mib.and_then(|m| i64::try_from(m).ok()),
                    link.cpus,
                    link.reserve_mib.and_then(|m| i64::try_from(m).ok()),
                    memory.and_then(|m| i64::try_from(m.available_mib).ok()),
                    memory.and_then(|m| m.psi_centi),
                    memory.map(|m| m.at.as_millis()),
                    memory.and_then(|m| m.psi_full_centi),
                    memory.and_then(|m| m.disk_free_mib.and_then(|d| i64::try_from(d).ok())),
                    memory.and_then(|m| m.disk_total_mib.and_then(|d| i64::try_from(d).ok())),
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
        let rows = stmt.query_map([user.get()], |r| Ok((r.get(17)?, link_row(r)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

/// The columns [`link_row`] reads, then the machine's name once `user_machines_v2 m` is joined.
const LINK_COLUMNS: &str =
    "machine_id, online, since, version, protocol, max_boxes, cache_gb, pool_features, mem_mib, cpus, \
     reserve_mib, mem_available_mib, psi_centi, reported_at, psi_full_centi, disk_free_mib, disk_total_mib, m.name \
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
        mem_mib: mib(r.get(8)?),
        cpus: r.get(9)?,
        reserve_mib: mib(r.get(10)?),
        memory: match (mib(r.get(11)?), r.get::<_, Option<i64>>(13)?) {
            (Some(available_mib), Some(at)) => Some(MemoryReport {
                at: Ts::from_millis(at),
                available_mib,
                total_mib: mib(r.get(8)?).unwrap_or(0),
                psi_centi: r.get(12)?,
                psi_full_centi: r.get(14)?,
                disk_free_mib: mib(r.get(15)?),
                disk_total_mib: mib(r.get(16)?),
            }),
            _ => None,
        },
    })
}

/// A stored MiB count; a negative one reads as unknown.
fn mib(stored: Option<i64>) -> Option<u64> {
    stored.and_then(|m| u64::try_from(m).ok())
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
