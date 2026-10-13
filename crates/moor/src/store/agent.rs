//! Agent session rows and the last hook report cached per task.

use moor_core::activity::{HookCache, HookKind, HookRecord};
use moor_core::domain::event::Event;
use moor_core::seam::clock::Ts;
use moor_core::TaskId;
use rusqlite::{params, OptionalExtension, Row, TransactionBehavior};
use serde_json::{json, Value};

use super::apply::insert_event;
use super::{load_task, Store, StoreError};

/// A stored agent session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionRow {
    /// Row id.
    pub(crate) id: i64,
    /// Owning task.
    pub(crate) task: TaskId,
    /// The agent's own session reference.
    pub(crate) session_ref: String,
    /// The agent's pane, if it has one.
    pub(crate) pane: Option<String>,
    /// When the session began.
    pub(crate) started_at: Ts,
    /// When the session ended; none while live.
    pub(crate) ended_at: Option<Ts>,
    /// Exit code, if the agent exited.
    pub(crate) exit: Option<i32>,
    /// Digest of the saved transcript.
    pub(crate) transcript_digest: Option<String>,
    /// The agent session id last registered for resume against this session's pane.
    pub(crate) registered_agent_session: Option<String>,
}

impl Store {
    /// The newest session of `task` that has not ended.
    pub(crate) fn live_session(&self, task: TaskId) -> Result<Option<SessionRow>, StoreError> {
        let conn = self.reader()?;
        Ok(conn
            .query_row(
                "SELECT id, task, session_ref, pane, started_at, ended_at, exit, transcript_digest, \
                 registered_agent_session FROM sessions WHERE task = ? AND ended_at IS NULL ORDER BY id DESC LIMIT 1",
                [task.get()],
                session_from_row,
            )
            .optional()?)
    }

    /// Whether `task` has ever had a session, ended or not.
    pub(crate) fn has_session(&self, task: TaskId) -> Result<bool, StoreError> {
        let conn = self.reader()?;
        Ok(conn
            .query_row(
                "SELECT 1 FROM sessions WHERE task = ? LIMIT 1",
                [task.get()],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Record the start of a session on `task` and its `SessionStarted` event; task state is not
    /// touched.
    pub(crate) fn begin_session(
        &self,
        task: TaskId,
        session_ref: &str,
        pane: Option<&str>,
    ) -> Result<i64, StoreError> {
        let now = self.now();
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if load_task(&tx, task)?.is_none() {
            return Err(StoreError::NotFound(task));
        }
        tx.execute(
            "INSERT INTO sessions (task, session_ref, pane, started_at) VALUES (?, ?, ?, ?)",
            params![task.get(), session_ref, pane, now.as_millis()],
        )?;
        let id = tx.last_insert_rowid();
        let started = Event::SessionStarted {
            session: session_ref.to_owned(),
        };
        insert_event(&tx, task, &started, now)?;
        tx.commit()?;
        drop(conn);
        Ok(id)
    }

    /// Record the end of a session; one that already ended is left untouched.
    pub(crate) fn end_session(
        &self,
        id: i64,
        exit: Option<i32>,
        transcript_digest: Option<&str>,
    ) -> Result<(), StoreError> {
        let now = self.now();
        let conn = self.writer()?;
        conn.execute(
            "UPDATE sessions SET ended_at = ?, exit = ?, transcript_digest = COALESCE(?, transcript_digest) \
             WHERE id = ? AND ended_at IS NULL",
            params![now.as_millis(), exit, transcript_digest, id],
        )?;
        drop(conn);
        Ok(())
    }

    /// Keep the agent session id registered for resume against session `id`.
    pub(crate) fn set_registered_session(&self, id: i64, agent: &str) -> Result<(), StoreError> {
        let conn = self.writer()?;
        conn.execute(
            "UPDATE sessions SET registered_agent_session = ? WHERE id = ?",
            params![agent, id],
        )?;
        drop(conn);
        Ok(())
    }

    /// Keep the digest of the transcript saved for a session that is still live.
    pub(crate) fn set_transcript_digest(&self, id: i64, digest: &str) -> Result<(), StoreError> {
        let conn = self.writer()?;
        conn.execute(
            "UPDATE sessions SET transcript_digest = ? WHERE id = ? AND ended_at IS NULL",
            params![digest, id],
        )?;
        drop(conn);
        Ok(())
    }

    /// Cache `record` as the latest hook report of `task`, stamped with the read time.
    pub(crate) fn put_hook(&self, task: TaskId, record: &HookRecord) -> Result<(), StoreError> {
        let now = self.now();
        let json = serde_json::to_string(&record_json(record))?;
        let conn = self.writer()?;
        conn.execute(
            "INSERT INTO hook_cache (task, record, at) VALUES (?1, ?2, ?3) \
             ON CONFLICT(task) DO UPDATE SET record = ?2, at = ?3",
            params![task.get(), json, now.as_millis()],
        )?;
        drop(conn);
        Ok(())
    }

    /// The latest cached hook report of `task`.
    pub(crate) fn hook(&self, task: TaskId) -> Result<Option<HookCache>, StoreError> {
        let conn = self.reader()?;
        let row: Option<(String, i64)> = conn
            .query_row(
                "SELECT record, at FROM hook_cache WHERE task = ?",
                [task.get()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        row.map(|(json, at)| {
            Ok(HookCache {
                record: record_from_json(&serde_json::from_str(&json)?)?,
                at: Ts::from_millis(at),
            })
        })
        .transpose()
    }
}

/// A session from a `sessions` row.
fn session_from_row(r: &Row<'_>) -> rusqlite::Result<SessionRow> {
    Ok(SessionRow {
        id: r.get(0)?,
        task: TaskId::new(r.get(1)?),
        session_ref: r.get(2)?,
        pane: r.get(3)?,
        started_at: Ts::from_millis(r.get(4)?),
        ended_at: r.get::<_, Option<i64>>(5)?.map(Ts::from_millis),
        exit: r.get(6)?,
        transcript_digest: r.get(7)?,
        registered_agent_session: r.get(8)?,
    })
}

/// The stored form of a hook report.
fn record_json(rec: &HookRecord) -> Value {
    let kind = match &rec.kind {
        HookKind::Starting => "starting",
        HookKind::Busy => "busy",
        HookKind::Idle => "idle",
        HookKind::Blocked => "blocked",
    };
    json!({
        "kind": kind,
        "message": rec.message,
        "session_id": rec.session_id,
        "at": rec.at.as_millis(),
    })
}

/// A hook report from its stored form.
fn record_from_json(v: &Value) -> Result<HookRecord, StoreError> {
    let bad = |what: &str| StoreError::Invalid(format!("hook cache: {what}"));
    let kind = match v["kind"].as_str() {
        Some("starting") => HookKind::Starting,
        Some("busy") => HookKind::Busy,
        Some("idle") => HookKind::Idle,
        Some("blocked") => HookKind::Blocked,
        _ => return Err(bad("unknown kind")),
    };
    Ok(HookRecord {
        kind,
        session_id: v["session_id"].as_str().map(str::to_owned),
        message: v["message"]
            .as_str()
            .or_else(|| v["waiting_for"].as_str())
            .map(str::to_owned),
        at: Ts::from_millis(v["at"].as_i64().ok_or_else(|| bad("missing at"))?),
    })
}

#[cfg(test)]
mod tests;
