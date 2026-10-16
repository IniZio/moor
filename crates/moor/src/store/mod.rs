//! `SQLite` store (WAL): one writer connection behind a mutex, fresh read-only connections for reads.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::Intent;
use moor_core::domain::task::TaskState;
use moor_core::seam::clock::{Clock, Ts};
use moor_core::view::LaunchFailure;
use moor_core::{BoxHandle, ProviderId, RunId, TaskId, TaskName};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, TransactionBehavior};

/// File name of the state database inside the state directory.
pub(crate) const STORE_FILE: &str = "moor.db";

#[cfg_attr(not(test), expect(dead_code, reason = "used by ops, not yet wired"))]
mod agent;
mod apply;
mod open;
mod proxy;
mod reads;
mod reports;
mod runs;
mod slack;
#[cfg(test)]
mod tests;

pub(crate) use agent::SessionRow;
pub(crate) use proxy::{Fenced, Lease, ProxyPorts, ProxyRow, ReadyProc, Reserve};
pub(crate) use runs::RunRow;
pub(crate) use runs::{NewRun, RunOutcome};
pub(crate) use slack::{SlackPrompt, SlackThread};

/// Schema version this build reads and writes.
pub(crate) const SCHEMA_VERSION: &str = "1";
/// Full schema, applied once when the file has no `meta` table.
const SCHEMA_SQL: &str = include_str!("schema.sql");
/// How long a connection waits on a lock before `SQLITE_BUSY`.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// Column list shared by the task reads; box columns come from a left join.
const TASK_SELECT: &str = "SELECT t.id, t.name, t.repo, t.issue, t.goal, t.base, t.branch, t.worktree, t.bound, \
    t.workspace_id, t.agent_session_id, t.state, t.state_reason, t.intent_kind, t.intent_pid, t.intent_run, \
    t.intent_at, t.setup_failed_at, t.created_at, t.updated_at, b.provider, b.handle, b.state, b.attempted_handle, b.workdir, \
    b.egress, b.sleep_reason, b.updated_at, t.idle_since, t.launch_failure FROM tasks t LEFT JOIN boxes b ON b.task = t.id";

/// Store failure.
#[derive(Debug, thiserror::Error)]
pub(crate) enum StoreError {
    /// `SQLite` failed.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// An event payload failed to (de)serialise.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// The state directory or a database file could not be prepared.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// The file was written by a different schema version.
    #[error("schema version `{found}` does not match `{want}`")]
    Schema {
        /// Version found in `meta`.
        found: String,
        /// Version this build needs.
        want: &'static str,
    },
    /// A task input or box input was illegal in the current state.
    #[error("{0}")]
    Illegal(#[from] moor_core::domain::Illegal),
    /// No such task.
    #[error("task {} not found", .0.get())]
    NotFound(TaskId),
    /// The decision or request was malformed.
    #[error("invalid: {0}")]
    Invalid(String),
    /// A stored value could not be parsed.
    #[error("corrupt store: {0}")]
    Corrupt(String),
    /// The writer mutex was poisoned.
    #[error("store writer poisoned")]
    Poisoned,
}

/// Box row joined to a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoxRow {
    /// Driver provider id.
    pub(crate) provider: ProviderId,
    /// Driver handle once acquired.
    pub(crate) handle: Option<BoxHandle>,
    /// Box state.
    pub(crate) state: BoxState,
    /// Handle of a failed acquire attempt, for cleanup.
    pub(crate) attempted_handle: Option<BoxHandle>,
    /// Working directory in the box.
    pub(crate) workdir: Option<String>,
    /// Box egress: `none`, `enforced` or `not_enforced`; unset when unknown.
    pub(crate) egress: Option<String>,
    /// Why the box cannot sleep, if the driver said so.
    pub(crate) sleep_reason: Option<String>,
    /// Last update.
    pub(crate) updated_at: Ts,
}

/// Task row with its optional box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskRow {
    /// Task id.
    pub(crate) id: TaskId,
    /// Task name.
    pub(crate) name: TaskName,
    /// Repo the task works on.
    pub(crate) repo: String,
    /// Issue reference.
    pub(crate) issue: Option<String>,
    /// Goal text.
    pub(crate) goal: Option<String>,
    /// Base ref.
    pub(crate) base: Option<String>,
    /// Branch name.
    pub(crate) branch: Option<String>,
    /// Worktree path.
    pub(crate) worktree: Option<String>,
    /// Worktree was supplied by the user and is never removed.
    pub(crate) bound: bool,
    /// Surface workspace id.
    pub(crate) workspace_id: Option<String>,
    /// The agent's own session id.
    pub(crate) agent_session_id: Option<String>,
    /// Task state.
    pub(crate) state: TaskState,
    /// Reason attached to the state.
    pub(crate) state_reason: Option<String>,
    /// In-flight operation, if any.
    pub(crate) intent: Option<Intent>,
    /// When the intent was recorded.
    pub(crate) intent_at: Option<Ts>,
    /// When setup last failed, if it has.
    pub(crate) setup_failed_at: Option<Ts>,
    /// When the task last became Idle; none while it is not Idle.
    pub(crate) idle_since: Option<Ts>,
    /// Why the last agent launch failed; cleared by a launch that succeeds.
    pub(crate) launch_failure: Option<LaunchFailure>,
    /// Creation time.
    pub(crate) created_at: Ts,
    /// Last update.
    pub(crate) updated_at: Ts,
    /// The task's box, if any.
    pub(crate) boxed: Option<BoxRow>,
}

/// Event row.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EventRow {
    /// Event id.
    pub(crate) id: i64,
    /// Owning task.
    pub(crate) task: TaskId,
    /// Event kind.
    pub(crate) kind: String,
    /// Event fields without `kind`.
    pub(crate) payload: serde_json::Value,
    /// Time recorded.
    pub(crate) ts: Ts,
}

/// The store: single writer, many readers.
pub(crate) struct Store {
    /// Database file, reopened for each read connection.
    path: PathBuf,
    /// The writer connection.
    writer: Mutex<Connection>,
    /// Time source for every timestamp.
    clock: Arc<dyn Clock>,
}

impl Store {
    /// Open (creating if needed) the store at `path`. The state directory is
    /// created 0700 and the db, `-wal` and `-shm` files are 0600.
    pub(crate) fn open(path: &Path, clock: Arc<dyn Clock>) -> Result<Self, StoreError> {
        open::prepare(path)?;
        let mut conn = Connection::open(path)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;",
        )?;
        init_schema(&mut conn)?;
        open::tighten(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            writer: Mutex::new(conn),
            clock,
        })
    }

    /// One task by id.
    pub(crate) fn task(&self, id: TaskId) -> Result<Option<TaskRow>, StoreError> {
        load_task(&self.reader()?, id)
    }

    /// Tasks that are neither closed nor rejected, by id ascending.
    pub(crate) fn open_tasks(&self) -> Result<Vec<TaskRow>, StoreError> {
        let conn = self.reader()?;
        let sql =
            format!("{TASK_SELECT} WHERE t.state NOT IN ('closed', 'rejected') ORDER BY t.id");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_and_then([], task_from_row)?;
        rows.collect()
    }

    /// The last `last` events of a task, ascending by id.
    pub(crate) fn events(&self, id: TaskId, last: usize) -> Result<Vec<EventRow>, StoreError> {
        let limit = i64::try_from(last).unwrap_or(i64::MAX);
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "SELECT id, task, kind, payload, ts FROM \
             (SELECT id, task, kind, payload, ts FROM events WHERE task = ?1 ORDER BY id DESC LIMIT ?2) ORDER BY id",
        )?;
        let rows = stmt.query_and_then((id.get(), limit), event_from_row)?;
        rows.collect()
    }

    /// The task's latest `needs_input`, `busy` or `working` event; none when it has had none.
    pub(crate) fn last_waiting_marker(&self, id: TaskId) -> Result<Option<EventRow>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "SELECT id, task, kind, payload, ts FROM events \
             WHERE task = ?1 AND kind IN ('needs_input', 'busy', 'working') ORDER BY id DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_and_then([id.get()], event_from_row)?;
        rows.next().transpose()
    }

    /// A new read-only connection; never blocks on the writer.
    pub(super) fn reader(&self) -> Result<Connection, StoreError> {
        let conn = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        Ok(conn)
    }

    /// Lock the writer connection.
    pub(super) fn writer(&self) -> Result<MutexGuard<'_, Connection>, StoreError> {
        self.writer.lock().map_err(|_| StoreError::Poisoned)
    }

    /// Current time from the clock seam.
    pub(super) fn now(&self) -> Ts {
        self.clock.now()
    }
}

/// Create the schema when `meta` is absent, else check the version.
fn init_schema(conn: &mut Connection) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let has_meta: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta')",
        [],
        |r| r.get(0),
    )?;
    if has_meta {
        let sql = "SELECT v FROM meta WHERE k = 'schema_version'";
        let found: Option<String> = tx.query_row(sql, [], |r| r.get(0)).optional()?;
        if found.as_deref() != Some(SCHEMA_VERSION) {
            return Err(StoreError::Schema {
                found: found.unwrap_or_else(|| "none".to_owned()),
                want: SCHEMA_VERSION,
            });
        }
    } else {
        tx.execute_batch(SCHEMA_SQL)?;
    }
    tx.execute_batch(slack::SLACK_SQL)?;
    tx.commit()?;
    Ok(())
}

/// Load one task joined to its box, on any connection (used inside `apply`'s transaction too).
pub(super) fn load_task(conn: &Connection, id: TaskId) -> Result<Option<TaskRow>, StoreError> {
    let sql = format!("{TASK_SELECT} WHERE t.id = ?1");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_and_then([id.get()], task_from_row)?;
    rows.next().transpose()
}

/// Parse a stored text value, naming the column on failure.
fn parsed<T, E: std::fmt::Display>(
    what: &str,
    raw: &str,
    f: impl FnOnce(&str) -> Result<T, E>,
) -> Result<T, StoreError> {
    f(raw).map_err(|e| StoreError::Corrupt(format!("{what} `{raw}`: {e}")))
}

/// Parse a stored enum value that has an `Option`-returning parser.
fn parsed_opt<T>(
    what: &str,
    raw: &str,
    f: impl FnOnce(&str) -> Option<T>,
) -> Result<T, StoreError> {
    parsed(what, raw, |s| f(s).ok_or("unknown value"))
}

/// Convert a stored integer to `u32`.
fn to_u32(what: &str, v: i64) -> Result<u32, StoreError> {
    u32::try_from(v).map_err(|_| StoreError::Corrupt(format!("{what} {v} out of range")))
}

/// Rebuild the intent from its three columns.
fn intent_from(
    kind: Option<String>,
    pid: Option<i64>,
    run: Option<i64>,
) -> Result<Option<Intent>, StoreError> {
    let Some(kind) = kind else { return Ok(None) };
    let pid = to_u32(
        "intent_pid",
        pid.ok_or_else(|| StoreError::Corrupt("intent without pid".into()))?,
    )?;
    match kind.as_str() {
        "provisioning" => Ok(Some(Intent::Provisioning { pid })),
        "setup" => Ok(Some(Intent::Setup { pid })),
        "running" => {
            let run =
                run.ok_or_else(|| StoreError::Corrupt("running intent without run".into()))?;
            Ok(Some(Intent::Running {
                run: RunId::new(run),
                pid,
            }))
        }
        other => Err(StoreError::Corrupt(format!("intent kind `{other}`"))),
    }
}

/// Map the joined box columns (offset 20..=27) to a `BoxRow`, if the box exists.
fn box_from_row(row: &Row<'_>) -> Result<Option<BoxRow>, StoreError> {
    let Some(provider) = row.get::<_, Option<String>>(20)? else {
        return Ok(None);
    };
    let state: String = row.get(22)?;
    Ok(Some(BoxRow {
        provider: ProviderId::new(provider),
        handle: row.get::<_, Option<String>>(21)?.map(BoxHandle::new),
        state: parsed_opt("box state", &state, BoxState::parse)?,
        attempted_handle: row.get::<_, Option<String>>(23)?.map(BoxHandle::new),
        workdir: row.get(24)?,
        egress: row.get(25)?,
        sleep_reason: row.get(26)?,
        updated_at: Ts::from_millis(row.get(27)?),
    }))
}

/// Map a `TASK_SELECT` row to a `TaskRow`.
fn task_from_row(row: &Row<'_>) -> Result<TaskRow, StoreError> {
    let name: String = row.get(1)?;
    let state: String = row.get(11)?;
    Ok(TaskRow {
        id: TaskId::new(row.get(0)?),
        name: parsed("task name", &name, TaskName::parse)?,
        repo: row.get(2)?,
        issue: row.get(3)?,
        goal: row.get(4)?,
        base: row.get(5)?,
        branch: row.get(6)?,
        worktree: row.get(7)?,
        bound: row.get::<_, i64>(8)? != 0,
        workspace_id: row.get(9)?,
        agent_session_id: row.get(10)?,
        state: parsed_opt("task state", &state, TaskState::parse)?,
        state_reason: row.get(12)?,
        intent: intent_from(row.get(13)?, row.get(14)?, row.get(15)?)?,
        intent_at: row.get::<_, Option<i64>>(16)?.map(Ts::from_millis),
        setup_failed_at: row.get::<_, Option<i64>>(17)?.map(Ts::from_millis),
        created_at: Ts::from_millis(row.get(18)?),
        updated_at: Ts::from_millis(row.get(19)?),
        idle_since: row.get::<_, Option<i64>>(28)?.map(Ts::from_millis),
        launch_failure: row
            .get::<_, Option<String>>(29)?
            .map(|j| serde_json::from_str(&j))
            .transpose()?,
        boxed: box_from_row(row)?,
    })
}

/// Map an events row to an `EventRow`.
fn event_from_row(row: &Row<'_>) -> Result<EventRow, StoreError> {
    let payload: String = row.get(3)?;
    Ok(EventRow {
        id: row.get(0)?,
        task: TaskId::new(row.get(1)?),
        kind: row.get(2)?,
        payload: serde_json::from_str(&payload)?,
        ts: Ts::from_millis(row.get(4)?),
    })
}
