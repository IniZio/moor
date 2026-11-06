//! Which enrolled machines hold a link to the runner, and the keys they prove themselves with.

use std::collections::HashMap;

use moor_core::name::BoxId;
use moor_core::request::{BoxHostMemory, MachineLink, MemoryReport};
use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_core::view::user::MachineKey;
use moor_schema::tables::{MachineBoxMemory, MachineLinks, UserMachinesV2};
use sea_query::{
    Asterisk, Expr, ExprTrait as _, Func, InsertStatement, Order, Query, SelectStatement,
};

use crate::store::db::{self, Db, DbExt as _, OptionalExtension};
use crate::store::{Store, StoreError};

impl Store {
    /// Every enrolled machine with the key its agent signs with, by id.
    pub(crate) fn machine_keys(&self) -> Result<Vec<MachineKey>, StoreError> {
        let conn = self.reader()?;
        let rows = conn.rows(
            Query::select()
                .columns([
                    UserMachinesV2::MachineId,
                    UserMachinesV2::User,
                    UserMachinesV2::Name,
                    UserMachinesV2::Pubkey,
                ])
                .from(UserMachinesV2::Table)
                .order_by(UserMachinesV2::MachineId, Order::Asc),
            |r| {
                Ok::<_, db::Error>(MachineKey {
                    machine_id: r.get(0)?,
                    user: UserId::new(r.get(1)?),
                    name: r.get(2)?,
                    pubkey: r.get(3)?,
                })
            },
        )?;
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
            let rows = tx.rows(
                link_select().and_where(
                    Expr::col((MachineLinks::Table, MachineLinks::MemAvailableMib)).is_not_null(),
                ),
                link_row,
            )?;
            rows.filter_map(|l| l.map(|l| l.memory.map(|m| (l.machine_id, m))).transpose())
                .collect::<Result<_, _>>()?
        };
        tx.run(Query::delete().from_table(MachineLinks::Table))?;
        for link in links {
            let memory = link
                .memory
                .or_else(|| reported.get(&link.machine_id).copied());
            tx.run(
                Query::delete()
                    .from_table(MachineLinks::Table)
                    .and_where(Expr::col(MachineLinks::MachineId).eq(link.machine_id)),
            )?;
            if !is_enrolled(&tx, link.machine_id)? {
                continue;
            }
            tx.run(link_insert(link, memory)?)?;
        }
        let held = {
            let rows = tx.rows(
                link_select().order_by((MachineLinks::Table, MachineLinks::MachineId), Order::Asc),
                link_row,
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        tx.commit()?;
        drop(conn);
        Ok(held)
    }

    /// Replaces the stored per-Box host memory with `boxes`, dropping any for a machine that is not
    /// enrolled or a Box another controller named.
    pub(crate) fn set_box_memory(&self, boxes: &[BoxHostMemory]) -> Result<(), StoreError> {
        let own = self.controller_id()?;
        let mut conn = self.writer();
        let tx = conn.transaction()?;
        tx.run(Query::delete().from_table(MachineBoxMemory::Table))?;
        for b in boxes.iter().filter(|b| b.controller == own) {
            tx.run(
                Query::delete()
                    .from_table(MachineBoxMemory::Table)
                    .and_where(Expr::col(MachineBoxMemory::MachineId).eq(b.machine_id))
                    .and_where(Expr::col(MachineBoxMemory::BoxId).eq(b.box_id.get())),
            )?;
            if !is_enrolled(&tx, b.machine_id)? {
                continue;
            }
            tx.run(
                Query::insert()
                    .into_table(MachineBoxMemory::Table)
                    .columns([
                        MachineBoxMemory::MachineId,
                        MachineBoxMemory::BoxId,
                        MachineBoxMemory::HostMemoryMib,
                        MachineBoxMemory::ResumeMemoryMib,
                    ])
                    .values_panic([
                        b.machine_id.into(),
                        b.box_id.get().into(),
                        i64::try_from(b.host_memory_mib).unwrap_or(i64::MAX).into(),
                        b.resume_memory_mib
                            .and_then(|m| i64::try_from(m).ok())
                            .into(),
                    ]),
            )?;
        }
        tx.commit()?;
        drop(conn);
        Ok(())
    }

    /// The MiB of host memory the Boxes of each machine that reported any hold, by machine id.
    pub(crate) fn box_memory_totals(&self) -> Result<HashMap<i64, u64>, StoreError> {
        let conn = self.reader()?;
        let rows = conn.rows(
            Query::select()
                .column(MachineBoxMemory::MachineId)
                .expr(Expr::col(MachineBoxMemory::HostMemoryMib).sum())
                .from(MachineBoxMemory::Table)
                .group_by_col(MachineBoxMemory::MachineId),
            |r| Ok::<_, db::Error>((r.get::<_, i64>(0)?, mib(r.get(1)?).unwrap_or(0))),
        )?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The MiB Box `id` needs to resume, as its provider last reported; none when it did not say.
    pub(crate) fn resume_memory_mib(&self, id: BoxId) -> Result<Option<u64>, StoreError> {
        let conn = self.reader()?;
        let stored = conn
            .row(
                Query::select()
                    .column(MachineBoxMemory::ResumeMemoryMib)
                    .from(MachineBoxMemory::Table)
                    .and_where(Expr::col(MachineBoxMemory::BoxId).eq(id.get())),
                |r| r.get(0),
            )
            .optional()?;
        Ok(mib(stored.flatten()))
    }

    /// The stored link of each of `user`'s machines that has one, by machine name.
    pub(crate) fn user_machine_links(
        &self,
        user: UserId,
    ) -> Result<Vec<(String, MachineLink)>, StoreError> {
        let conn = self.reader()?;
        let rows = conn.rows(
            link_select()
                .and_where(Expr::col((UserMachinesV2::Table, UserMachinesV2::User)).eq(user.get())),
            |r| Ok::<_, db::Error>((r.get(17)?, link_row(r)?)),
        )?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

/// The insert of `link`, with the last `memory` its machine reported.
fn link_insert(
    link: &MachineLink,
    memory: Option<MemoryReport>,
) -> Result<InsertStatement, StoreError> {
    Ok(Query::insert()
        .into_table(MachineLinks::Table)
        .columns([
            MachineLinks::MachineId,
            MachineLinks::Online,
            MachineLinks::Since,
            MachineLinks::Version,
            MachineLinks::Protocol,
            MachineLinks::MaxBoxes,
            MachineLinks::CacheGb,
            MachineLinks::PoolFeatures,
            MachineLinks::MemMib,
            MachineLinks::Cpus,
            MachineLinks::ReserveMib,
            MachineLinks::MemAvailableMib,
            MachineLinks::PsiCenti,
            MachineLinks::ReportedAt,
            MachineLinks::PsiFullCenti,
            MachineLinks::DiskFreeMib,
            MachineLinks::DiskTotalMib,
        ])
        .values_panic([
            link.machine_id.into(),
            link.online.into(),
            link.since.as_millis().into(),
            link.version.clone().into(),
            link.protocol.into(),
            link.max_boxes.into(),
            link.cache_gb.into(),
            serde_json::to_string(&link.features)
                .map_err(|e| StoreError::Corrupt(format!("link features: {e}")))?
                .into(),
            link.mem_mib.and_then(|m| i64::try_from(m).ok()).into(),
            link.cpus.into(),
            link.reserve_mib.and_then(|m| i64::try_from(m).ok()).into(),
            memory
                .and_then(|m| i64::try_from(m.available_mib).ok())
                .into(),
            memory.and_then(|m| m.psi_centi).into(),
            memory.map(|m| m.at.as_millis()).into(),
            memory.and_then(|m| m.psi_full_centi).into(),
            memory
                .and_then(|m| m.disk_free_mib.and_then(|d| i64::try_from(d).ok()))
                .into(),
            memory
                .and_then(|m| m.disk_total_mib.and_then(|d| i64::try_from(d).ok()))
                .into(),
        ])
        .take())
}

/// Whether `machine_id` is an enrolled machine.
fn is_enrolled(conn: &dyn Db, machine_id: i64) -> Result<bool, StoreError> {
    let found = conn
        .row(
            Query::select()
                .expr(Func::count(Expr::col(Asterisk)))
                .from(UserMachinesV2::Table)
                .and_where(Expr::col(UserMachinesV2::MachineId).eq(machine_id)),
            |r| r.get::<_, i64>(0),
        )
        .optional()?;
    Ok(found.is_some_and(|n| n > 0))
}

/// The columns [`link_row`] reads, then the machine's name, from the links and their machines.
fn link_select() -> SelectStatement {
    let link = |c| (MachineLinks::Table, c);
    Query::select()
        .columns(
            [
                MachineLinks::MachineId,
                MachineLinks::Online,
                MachineLinks::Since,
                MachineLinks::Version,
                MachineLinks::Protocol,
                MachineLinks::MaxBoxes,
                MachineLinks::CacheGb,
                MachineLinks::PoolFeatures,
                MachineLinks::MemMib,
                MachineLinks::Cpus,
                MachineLinks::ReserveMib,
                MachineLinks::MemAvailableMib,
                MachineLinks::PsiCenti,
                MachineLinks::ReportedAt,
                MachineLinks::PsiFullCenti,
                MachineLinks::DiskFreeMib,
                MachineLinks::DiskTotalMib,
            ]
            .map(link),
        )
        .column((UserMachinesV2::Table, UserMachinesV2::Name))
        .from(MachineLinks::Table)
        .left_join(
            UserMachinesV2::Table,
            Expr::col((UserMachinesV2::Table, UserMachinesV2::MachineId))
                .equals(link(MachineLinks::MachineId)),
        )
        .take()
}

/// One `machine_links` row.
fn link_row(r: &db::Row<'_>) -> db::Result<MachineLink> {
    Ok(MachineLink {
        machine_id: r.get(0)?,
        online: r.get(1)?,
        since: Ts::from_millis(r.get(2)?),
        version: r.get(3)?,
        protocol: r.get(4)?,
        max_boxes: r.get(5)?,
        cache_gb: r.get(6)?,
        features: serde_json::from_str(&r.get::<_, String>(7)?).map_err(|e| {
            db::Error::Convert {
                index: 7,
                source: Box::new(e),
            }
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
    conn: &dyn Db,
    user: UserId,
) -> Result<Vec<(i64, moor_core::config::UserMachine)>, StoreError> {
    let rows = conn.rows(
        Query::select()
            .columns([
                UserMachinesV2::MachineId,
                UserMachinesV2::Name,
                UserMachinesV2::Labels,
                UserMachinesV2::Capacity,
                UserMachinesV2::PoolProvider,
            ])
            .from(UserMachinesV2::Table)
            .and_where(Expr::col(UserMachinesV2::User).eq(user.get()))
            .order_by(UserMachinesV2::Name, Order::Asc),
        |r| {
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
        },
    )?;
    rows.collect()
}
