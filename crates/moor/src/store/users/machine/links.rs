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
                "INSERT OR REPLACE INTO machine_links (machine_id, online, since) \
                 SELECT machine_id, ?, ? FROM user_machines_v2 WHERE machine_id = ?",
                params![link.online, link.since.as_millis(), link.machine_id],
            )?;
        }
        let held = {
            let mut stmt = tx.prepare(
                "SELECT machine_id, online, since FROM machine_links ORDER BY machine_id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(MachineLink {
                    machine_id: r.get(0)?,
                    online: r.get(1)?,
                    since: Ts::from_millis(r.get(2)?),
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        tx.commit()?;
        drop(conn);
        Ok(held)
    }
}
