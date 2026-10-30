//! `SQLite` store (WAL): one writer connection behind a mutex, fresh read-only connections for reads.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::{BoxRepo, Intent};
use moor_core::domain::session_state::SessionState;
use moor_core::request::Pins;
use moor_core::seam::clock::{Clock, Ts};
use moor_core::shape::Shape;
use moor_core::user::UserId;
use moor_core::view::LaunchFailure;
use moor_core::{BoxHandle, BoxId, BoxName, ProviderId, RunId};
use repo::repo_from_row;
use rusqlite::{Connection, OpenFlags, Row};

use driver_row::driver_from_row;
use init::{parsed, parsed_opt};

pub(crate) use error::StoreError;

/// File name of the state database inside the state directory.
pub(crate) const STORE_FILE: &str = "moor.db";

#[cfg_attr(not(test), expect(dead_code, reason = "used by ops, not yet wired"))]
mod agent;
mod apply;
pub(crate) mod artifacts;
mod bindings;
pub(crate) mod box_profile;
mod driver_row;
mod error;
pub(crate) use bindings::Binding;
mod follow;
#[cfg(test)]
pub(crate) use follow::FollowClaim;
pub(crate) mod host;
mod init;
pub(crate) mod lease;
mod open;
mod proxy;
pub(super) mod reads;
mod repo;
mod reports;
mod runs;
mod sessions;
#[cfg(test)]
mod tests;
mod turns;
mod users;

pub(crate) use agent::SessionRow;
pub(crate) use proxy::{Fenced, Lease, ProxyRow, ReadyProc, Reserve};
pub(crate) use proxy::{TunnelAck, TunnelRequest, TunnelWant};
#[cfg(test)]
pub(crate) use proxy::{EGRESS_PORT_MAX, EGRESS_PORT_MIN};
pub(crate) use runs::{NewRun, RunOutcome, RunRow};
pub(crate) use sessions::SessionRecord;
pub(crate) use users::machine::MachineWake;
pub(crate) use users::mcp_oauth::{GrantRow, SignIn};

/// How long a connection waits on a lock before `SQLITE_BUSY`.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// Column list shared by the box reads; session columns come from the current session.
const BOX_SELECT: &str = "SELECT b.id, b.name, b.repo, b.issue, b.prompt, b.base, b.branch, \
    s.workspace_id, s.agent_session_id, b.state, b.state_reason, b.intent_kind, b.intent_pid, b.intent_run, \
    b.intent_at, b.setup_failed_at, b.created_at, b.updated_at, b.provider, b.handle, b.driver_state, \
    b.attempted_handle, b.workdir, b.egress, b.sleep_reason, b.driver_updated_at, b.idle_since, b.launch_failure, \
    s.state, b.user, b.repo_config, b.author_name, b.author_email, b.cpus, b.mem_mib FROM boxes b LEFT JOIN sessions s ON s.box = b.id \
    AND s.ordinal = (SELECT MAX(ordinal) FROM sessions WHERE box = b.id)";

/// Box row joined to a box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DriverRow {
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
    /// CPUs and memory the box delivers, as read inside it; `None` until a start has read them.
    pub(crate) delivered: Option<Shape>,
    /// Last update.
    pub(crate) updated_at: Ts,
}

/// Box row with its optional box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoxRow {
    /// Box id.
    pub(crate) id: BoxId,
    /// Box name.
    pub(crate) name: BoxName,
    /// The repo the Box clones, with its config and author at the base; `None` for no repo.
    pub(crate) repo: Option<BoxRepo>,
    /// The user the Box acts for; every Box `start` makes has one.
    pub(crate) user: Option<UserId>,
    /// Issue reference.
    pub(crate) issue: Option<String>,
    /// Prompt text.
    pub(crate) prompt: Option<String>,
    /// Base ref.
    pub(crate) base: Option<String>,
    /// Branch name.
    pub(crate) branch: Option<String>,
    /// Surface workspace id.
    pub(crate) workspace_id: Option<String>,
    /// The agent's own session id.
    pub(crate) agent_session_id: Option<String>,
    /// The Box's lifecycle.
    pub(crate) lifecycle: BoxLifecycle,
    /// The primary Session's state.
    pub(crate) session: SessionState,
    /// Reason attached to the state.
    pub(crate) state_reason: Option<String>,
    /// In-flight operation, if any.
    pub(crate) intent: Option<Intent>,
    /// When the intent was recorded.
    pub(crate) intent_at: Option<Ts>,
    /// When setup last failed, if it has.
    pub(crate) setup_failed_at: Option<Ts>,
    /// When the box last became Idle; none while it is not Idle.
    pub(crate) idle_since: Option<Ts>,
    /// Why the last agent launch failed; cleared by a launch that succeeds.
    pub(crate) launch_failure: Option<LaunchFailure>,
    /// Creation time.
    pub(crate) created_at: Ts,
    /// Last update.
    pub(crate) updated_at: Ts,
    /// The box's box, if any.
    pub(crate) boxed: Option<DriverRow>,
}

/// Event row.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EventRow {
    /// Event id.
    pub(crate) id: i64,
    /// Owning box.
    pub(crate) box_id: BoxId,
    /// Event kind.
    pub(crate) kind: String,
    /// Event fields without `kind`.
    pub(crate) payload: serde_json::Value,
    /// Time recorded.
    pub(crate) ts: Ts,
}

/// The store: single writer, many readers. A clone shares the writer connection.
#[derive(Clone)]
pub(crate) struct Store {
    /// Database file, reopened for each read connection.
    path: PathBuf,
    /// The writer connection.
    writer: Arc<Mutex<Connection>>,
    /// Time source for every timestamp.
    clock: Arc<dyn Clock>,
    /// The Box each name meant when the controller let the request name it. A name lookup
    /// returns that Box only, so a Box of the same name made later is never acted on.
    pins: Pins,
}

impl Store {
    /// Open (creating if needed) the store at `path`: state dir 0700, db, `-wal` and `-shm` 0600.
    /// A db this build would not migrate is refused before anything writes to it or changes a mode.
    pub(crate) fn open(path: &Path, clock: Arc<dyn Clock>) -> Result<Self, StoreError> {
        init::refuse_other_version(path)?;
        open::prepare(path)?;
        let mut conn = Connection::open(path)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;",
        )?;
        init::migrate(&mut conn, path)?;
        open::tighten(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            writer: Arc::new(Mutex::new(conn)),
            clock,
            pins: Pins::new(),
        })
    }

    /// One box by id.
    pub(crate) fn box_row(&self, id: BoxId) -> Result<Option<BoxRow>, StoreError> {
        load_box(&self.reader()?, id)
    }

    /// The last `last` events of a box, ascending by id.
    pub(crate) fn events(&self, id: BoxId, last: usize) -> Result<Vec<EventRow>, StoreError> {
        let limit = i64::try_from(last).unwrap_or(i64::MAX);
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "SELECT id, box, kind, payload, ts FROM \
             (SELECT id, box, kind, payload, ts FROM events WHERE box = ?1 ORDER BY id DESC LIMIT ?2) ORDER BY id",
        )?;
        let rows = stmt.query_and_then((id.get(), limit), event_from_row)?;
        rows.collect()
    }

    /// The box's latest `needs_input`, `busy` or `working` event; none when it has had none.
    pub(crate) fn last_waiting_marker(&self, id: BoxId) -> Result<Option<EventRow>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "SELECT id, box, kind, payload, ts FROM events \
             WHERE box = ?1 AND kind IN ('needs_input', 'busy', 'working') ORDER BY id DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_and_then([id.get()], event_from_row)?;
        rows.next().transpose()
    }

    /// The box's newest event of `kind`, however long ago; none when it has had none.
    pub(crate) fn newest_event_of(
        &self,
        id: BoxId,
        kind: &str,
    ) -> Result<Option<EventRow>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "SELECT id, box, kind, payload, ts FROM events \
             WHERE box = ?1 AND kind = ?2 ORDER BY id DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_and_then((id.get(), kind), event_from_row)?;
        rows.next().transpose()
    }

    /// When Box `id`'s machine last came up, by acquire or wake.
    pub(crate) fn awake_since(&self, id: BoxId) -> Result<Option<Ts>, StoreError> {
        let conn = self.reader()?;
        let ts = conn
            .query_row(
                "SELECT MAX(ts) FROM events WHERE box = ?1 AND kind = 'box_ready'",
                [id.get()],
                |r| r.get::<_, Option<i64>>(0),
            )?
            .map(Ts::from_millis);
        Ok(ts)
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

    /// Lock the writer connection. A thread that panicked while holding it left at worst an
    /// open transaction, which rusqlite rolled back during unwind, so the connection is sound.
    pub(super) fn writer(&self) -> MutexGuard<'_, Connection> {
        self.writer.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Current time from the clock seam.
    pub(super) fn now(&self) -> Ts {
        self.clock.now()
    }
}

/// Load one box joined to its box, on any connection (used inside `apply`'s transaction too).
pub(super) fn load_box(conn: &Connection, id: BoxId) -> Result<Option<BoxRow>, StoreError> {
    let mut stmt = conn.prepare(&format!("{BOX_SELECT} WHERE b.id = ?1"))?;
    let mut rows = stmt.query_and_then([id.get()], box_from_row)?;
    rows.next().transpose()
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

/// Map a `BOX_SELECT` row to a `BoxRow`.
fn box_from_row(row: &Row<'_>) -> Result<BoxRow, StoreError> {
    let name: String = row.get(1)?;
    let lifecycle: String = row.get(9)?;
    let session: Option<String> = row.get(28)?;
    let session = match session {
        Some(raw) => parsed_opt("session state", &raw, SessionState::parse)?,
        None => SessionState::Starting,
    };
    let lifecycle = parsed_opt("box state", &lifecycle, BoxLifecycle::parse)?;
    Ok(BoxRow {
        id: BoxId::new(row.get(0)?),
        name: parsed("box name", &name, BoxName::parse_allow_bare)?,
        repo: repo_from_row(row)?,
        user: row.get::<_, Option<i64>>(29)?.map(UserId::new),
        issue: row.get(3)?,
        prompt: row.get(4)?,
        base: row.get(5)?,
        branch: row.get(6)?,
        workspace_id: row.get(7)?,
        agent_session_id: row.get(8)?,
        lifecycle,
        session,
        state_reason: row.get(10)?,
        intent: intent_from(row.get(11)?, row.get(12)?, row.get(13)?)?,
        intent_at: row.get::<_, Option<i64>>(14)?.map(Ts::from_millis),
        setup_failed_at: row.get::<_, Option<i64>>(15)?.map(Ts::from_millis),
        created_at: Ts::from_millis(row.get(16)?),
        updated_at: Ts::from_millis(row.get(17)?),
        idle_since: row.get::<_, Option<i64>>(26)?.map(Ts::from_millis),
        launch_failure: row
            .get::<_, Option<String>>(27)?
            .map(|j| serde_json::from_str(&j))
            .transpose()?,
        boxed: driver_from_row(row)?,
    })
}

/// Map an events row to an `EventRow`.
fn event_from_row(row: &Row<'_>) -> Result<EventRow, StoreError> {
    let payload: String = row.get(3)?;
    Ok(EventRow {
        id: row.get(0)?,
        box_id: BoxId::new(row.get(1)?),
        kind: row.get(2)?,
        payload: serde_json::from_str(&payload)?,
        ts: Ts::from_millis(row.get(4)?),
    })
}
