//! Records of Box resources, written before creation so cleanup works from a record.

#![cfg_attr(
    not(test),
    expect(dead_code, reason = "read by Box release, not yet wired")
)]

use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use moor_schema::tables::BoxLease;
use sea_query::{Expr, ExprTrait as _, OnConflict, Order, Query, SelectStatement, Value};

use super::{parsed_opt, Store, StoreError};
use crate::store::db::{Db, DbExt as _, Row};

/// Columns read into a `LeaseRow`, in `row` order.
const COLUMNS: [BoxLease; 12] = [
    BoxLease::Id,
    BoxLease::Box,
    BoxLease::Kind,
    BoxLease::Key,
    BoxLease::State,
    BoxLease::Booted,
    BoxLease::Pgid,
    BoxLease::Sid,
    BoxLease::RootPid,
    BoxLease::RootStart,
    BoxLease::SizeAtRelease,
    BoxLease::UpdatedAt,
];

/// What a leased resource is; `key` means a path, UDID, project or tag by kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LeaseKind {
    /// The Box root directory.
    BoxRoot,
    /// A directory inside the Box.
    BoxDir,
    /// A home directory made for the Box.
    Home,
    /// A generated xcconfig file.
    Xcconfig,
    /// A simulator device set.
    SimSet,
    /// A single simulator device.
    SimDevice,
    /// A spawned process tree.
    Process,
    /// A compose project.
    Compose,
    /// A cache directory.
    Cache,
}

impl LeaseKind {
    /// The stored name.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::BoxRoot => "box_root",
            Self::BoxDir => "box_dir",
            Self::Home => "home",
            Self::Xcconfig => "xcconfig",
            Self::SimSet => "sim_set",
            Self::SimDevice => "sim_device",
            Self::Process => "process",
            Self::Compose => "compose",
            Self::Cache => "cache",
        }
    }

    /// The kind stored under `s`, if any.
    pub(crate) fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "box_root" => Self::BoxRoot,
            "box_dir" => Self::BoxDir,
            "home" => Self::Home,
            "xcconfig" => Self::Xcconfig,
            "sim_set" => Self::SimSet,
            "sim_device" => Self::SimDevice,
            "process" => Self::Process,
            "compose" => Self::Compose,
            "cache" => Self::Cache,
            _ => return None,
        })
    }
}

/// How far a resource got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LeaseState {
    /// Recorded; creation may not have happened.
    Planned,
    /// Creation succeeded.
    Held,
}

impl LeaseState {
    /// The stored name.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Held => "held",
        }
    }

    /// The state stored under `s`, if any.
    fn parse(s: &str) -> Option<Self> {
        match s {
            "planned" => Some(Self::Planned),
            "held" => Some(Self::Held),
            _ => None,
        }
    }
}

/// Row id of a lease.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LeaseId(pub(crate) i64);

/// Identity of a process tree, enough to tell it from a recycled pid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProcessIds {
    /// Process group id.
    pub pgid: i64,
    /// Session id.
    pub sid: i64,
    /// Root process id.
    pub root_pid: i64,
    /// Start time of the root process.
    pub root_start: i64,
}

/// A stored lease.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LeaseRow {
    /// Row id.
    pub id: LeaseId,
    /// Owning Box.
    pub box_id: BoxId,
    /// Resource kind.
    pub kind: LeaseKind,
    /// Path, UDID, compose project or process tag.
    pub key: String,
    /// Planned or held.
    pub state: LeaseState,
    /// Whether a simulator was booted.
    pub booted: bool,
    /// Process identity, once recorded.
    pub process: Option<ProcessIds>,
    /// Bytes on disk when released, if measured.
    pub size_at_release: Option<u64>,
    /// Last change time.
    pub updated_at: Ts,
}

impl Store {
    /// Record a resource as `planned` before creating it. Returns the existing id, state
    /// unchanged, when `(box, kind, key)` is already recorded.
    pub(crate) fn plan_lease(
        &self,
        box_id: BoxId,
        kind: LeaseKind,
        key: &str,
    ) -> Result<LeaseId, StoreError> {
        let now = self.now().as_millis();
        let conn = self.writer();
        conn.run(
            Query::insert()
                .into_table(BoxLease::Table)
                .columns([
                    BoxLease::Box,
                    BoxLease::Kind,
                    BoxLease::Key,
                    BoxLease::State,
                    BoxLease::CreatedAt,
                    BoxLease::UpdatedAt,
                ])
                .values_panic([
                    box_id.get().into(),
                    kind.as_str().into(),
                    key.into(),
                    LeaseState::Planned.as_str().into(),
                    now.into(),
                    now.into(),
                ])
                .on_conflict(OnConflict::new().do_nothing().to_owned()),
        )?;
        let id = conn.row(
            Query::select()
                .column(BoxLease::Id)
                .from(BoxLease::Table)
                .and_where(Expr::col(BoxLease::Box).eq(box_id.get()))
                .and_where(Expr::col(BoxLease::Kind).eq(kind.as_str()))
                .and_where(Expr::col(BoxLease::Key).eq(key)),
            |r| r.get(0),
        )?;
        drop(conn);
        Ok(LeaseId(id))
    }

    /// Mark the resource created; returns whether the row exists.
    pub(crate) fn hold_lease(&self, id: LeaseId) -> Result<bool, StoreError> {
        self.update(id, &[(BoxLease::State, LeaseState::Held.as_str().into())])
    }

    /// Record whether a simulator is booted; returns whether the row exists.
    pub(crate) fn set_booted(&self, id: LeaseId, booted: bool) -> Result<bool, StoreError> {
        self.update(id, &[(BoxLease::Booted, i64::from(booted).into())])
    }

    /// Record process identity; returns whether the row exists.
    pub(crate) fn record_process(&self, id: LeaseId, ids: ProcessIds) -> Result<bool, StoreError> {
        self.update(
            id,
            &[
                (BoxLease::Pgid, ids.pgid.into()),
                (BoxLease::Sid, ids.sid.into()),
                (BoxLease::RootPid, ids.root_pid.into()),
                (BoxLease::RootStart, ids.root_start.into()),
            ],
        )
    }

    /// Delete the row; call only after release is verified. Returns whether a row was deleted.
    pub(crate) fn drop_lease(&self, id: LeaseId) -> Result<bool, StoreError> {
        let conn = self.writer();
        let changed = conn.run(
            Query::delete()
                .from_table(BoxLease::Table)
                .and_where(Expr::col(BoxLease::Id).eq(id.0)),
        )?;
        drop(conn);
        Ok(changed > 0)
    }

    /// Delete every lease row of `box_id`; call only once its machine is released or never
    /// existed. Returns how many rows were deleted.
    pub(crate) fn drop_box_leases(&self, box_id: BoxId) -> Result<usize, StoreError> {
        let conn = self.writer();
        let changed = conn.run(
            Query::delete()
                .from_table(BoxLease::Table)
                .and_where(Expr::col(BoxLease::Box).eq(box_id.get())),
        )?;
        drop(conn);
        Ok(changed)
    }

    /// Leases of `box_id`, by id.
    pub(crate) fn leases(&self, box_id: BoxId) -> Result<Vec<LeaseRow>, StoreError> {
        let conn = self.reader()?;
        query(
            &conn,
            Query::select()
                .columns(COLUMNS)
                .from(BoxLease::Table)
                .and_where(Expr::col(BoxLease::Box).eq(box_id.get()))
                .order_by(BoxLease::Id, Order::Asc),
        )
    }

    /// Every lease, by id.
    pub(crate) fn all_leases(&self) -> Result<Vec<LeaseRow>, StoreError> {
        let conn = self.reader()?;
        query(
            &conn,
            Query::select()
                .columns(COLUMNS)
                .from(BoxLease::Table)
                .order_by(BoxLease::Id, Order::Asc),
        )
    }

    /// Assign `sets` on the row and stamp `updated_at`.
    fn update(&self, id: LeaseId, sets: &[(BoxLease, Value)]) -> Result<bool, StoreError> {
        let now = self.now().as_millis();
        let mut q = Query::update();
        q.table(BoxLease::Table);
        for (c, v) in sets {
            q.value(*c, Expr::val(v.clone()));
        }
        let conn = self.writer();
        let changed = conn.run(
            q.value(BoxLease::UpdatedAt, now)
                .and_where(Expr::col(BoxLease::Id).eq(id.0)),
        )?;
        drop(conn);
        Ok(changed > 0)
    }
}

/// Run `q` and read every row as a `LeaseRow`.
fn query(conn: &dyn Db, q: &mut SelectStatement) -> Result<Vec<LeaseRow>, StoreError> {
    conn.rows(q, row)?.collect()
}

/// Read one `LeaseRow` laid out as `COLUMNS`.
fn row(r: &Row<'_>) -> Result<LeaseRow, StoreError> {
    let kind: String = r.get(2)?;
    let state: String = r.get(4)?;
    let pgid: Option<i64> = r.get(6)?;
    let sid: Option<i64> = r.get(7)?;
    let root_pid: Option<i64> = r.get(8)?;
    let root_start: Option<i64> = r.get(9)?;
    let process = match (pgid, sid, root_pid, root_start) {
        (Some(pgid), Some(sid), Some(root_pid), Some(root_start)) => Some(ProcessIds {
            pgid,
            sid,
            root_pid,
            root_start,
        }),
        _ => None,
    };
    let size: Option<i64> = r.get(10)?;
    Ok(LeaseRow {
        id: LeaseId(r.get(0)?),
        box_id: BoxId::new(r.get(1)?),
        kind: parsed_opt("lease kind", &kind, LeaseKind::parse)?,
        key: r.get(3)?,
        state: parsed_opt("lease state", &state, LeaseState::parse)?,
        booted: r.get::<_, i64>(5)? != 0,
        process,
        size_at_release: size.and_then(|s| u64::try_from(s).ok()),
        updated_at: Ts::from_millis(r.get(11)?),
    })
}

#[cfg(test)]
mod tests;
