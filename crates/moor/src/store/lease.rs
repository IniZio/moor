//! Records of Box resources, written before creation so cleanup works from a record.

#![cfg_attr(
    not(test),
    expect(dead_code, reason = "read by Box release, not yet wired")
)]

use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use rusqlite::{params, Connection, Row};

use super::{parsed_opt, Store, StoreError};

/// DDL for the lease table; idempotent so existing files gain it on open.
pub(super) const LEASE_SQL: &str = "\
CREATE TABLE IF NOT EXISTS box_lease (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  box INTEGER NOT NULL REFERENCES boxes(id),
  kind TEXT NOT NULL,
  key TEXT NOT NULL,
  state TEXT NOT NULL,
  booted INTEGER NOT NULL DEFAULT 0,
  pgid INTEGER, sid INTEGER, root_pid INTEGER, root_start INTEGER,
  size_at_release INTEGER,
  created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
  UNIQUE(box, kind, key));
CREATE INDEX IF NOT EXISTS box_lease_box ON box_lease(box);
";

/// Columns read into a `LeaseRow`, in `row` order.
const COLUMNS: &str = "id, box, kind, key, state, booted, pgid, sid, root_pid, root_start, \
                       size_at_release, updated_at";

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
    /// Owning Task.
    pub task: BoxId,
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
        task: BoxId,
        kind: LeaseKind,
        key: &str,
    ) -> Result<LeaseId, StoreError> {
        let now = self.now().as_millis();
        let conn = self.writer()?;
        conn.execute(
            "INSERT OR IGNORE INTO box_lease (box, kind, key, state, created_at, updated_at) \
             VALUES (?, ?, ?, 'planned', ?, ?)",
            params![task.get(), kind.as_str(), key, now, now],
        )?;
        let id = conn.query_row(
            "SELECT id FROM box_lease WHERE box = ? AND kind = ? AND key = ?",
            params![task.get(), kind.as_str(), key],
            |r| r.get(0),
        )?;
        drop(conn);
        Ok(LeaseId(id))
    }

    /// Mark the resource created; returns whether the row exists.
    pub(crate) fn hold_lease(&self, id: LeaseId) -> Result<bool, StoreError> {
        self.update(id, "state = ?", params![LeaseState::Held.as_str()])
    }

    /// Record whether a simulator is booted; returns whether the row exists.
    pub(crate) fn set_booted(&self, id: LeaseId, booted: bool) -> Result<bool, StoreError> {
        self.update(id, "booted = ?", params![i64::from(booted)])
    }

    /// Record process identity; returns whether the row exists.
    pub(crate) fn record_process(&self, id: LeaseId, ids: ProcessIds) -> Result<bool, StoreError> {
        self.update(
            id,
            "pgid = ?, sid = ?, root_pid = ?, root_start = ?",
            params![ids.pgid, ids.sid, ids.root_pid, ids.root_start],
        )
    }

    /// Delete the row; call only after release is verified. Returns whether a row was deleted.
    pub(crate) fn drop_lease(&self, id: LeaseId) -> Result<bool, StoreError> {
        let conn = self.writer()?;
        let changed = conn.execute("DELETE FROM box_lease WHERE id = ?", [id.0])?;
        drop(conn);
        Ok(changed > 0)
    }

    /// Leases of `task`, by id.
    pub(crate) fn leases(&self, task: BoxId) -> Result<Vec<LeaseRow>, StoreError> {
        let conn = self.reader()?;
        query(
            &conn,
            &format!("SELECT {COLUMNS} FROM box_lease WHERE box = ? ORDER BY id"),
            params![task.get()],
        )
    }

    /// Every lease, by id.
    pub(crate) fn all_leases(&self) -> Result<Vec<LeaseRow>, StoreError> {
        let conn = self.reader()?;
        query(
            &conn,
            &format!("SELECT {COLUMNS} FROM box_lease ORDER BY id"),
            params![],
        )
    }

    /// Apply `set` to the row and stamp `updated_at`; `args` bind the placeholders in `set`.
    fn update(
        &self,
        id: LeaseId,
        set: &str,
        args: &[&dyn rusqlite::ToSql],
    ) -> Result<bool, StoreError> {
        let now = self.now().as_millis();
        let mut all: Vec<&dyn rusqlite::ToSql> = args.to_vec();
        all.push(&now);
        all.push(&id.0);
        let conn = self.writer()?;
        let changed = conn.execute(
            &format!("UPDATE box_lease SET {set}, updated_at = ? WHERE id = ?"),
            all.as_slice(),
        )?;
        drop(conn);
        Ok(changed > 0)
    }
}

/// Run `sql` and read every row as a `LeaseRow`.
fn query(
    conn: &Connection,
    sql: &str,
    args: impl rusqlite::Params,
) -> Result<Vec<LeaseRow>, StoreError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_and_then(args, row)?;
    rows.collect()
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
        task: BoxId::new(r.get(1)?),
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
