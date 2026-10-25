//! Agent session rows and the last hook report cached per box.

use moor_core::activity::{HookCache, HookKind, HookRecord};
use moor_core::domain::event::Event;
use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use rusqlite::{params, OptionalExtension, Row, TransactionBehavior};
use serde_json::{json, Value};

use super::apply::insert_event;
use super::{load_box, Store, StoreError};

/// A stored agent session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionRow {
    /// Row id.
    pub(crate) id: i64,
    /// Owning box.
    pub(crate) box_id: BoxId,
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
    /// The Box's Session current when the agent started.
    pub(crate) session: Option<i64>,
}

impl Store {
    /// The newest session of `box_id` that has not ended.
    pub(crate) fn live_session(&self, box_id: BoxId) -> Result<Option<SessionRow>, StoreError> {
        let conn = self.reader()?;
        Ok(conn
            .query_row(
                "SELECT id, box, session_ref, pane, started_at, ended_at, exit, transcript_digest, \
                 registered_agent_session, session FROM launches WHERE box = ? AND ended_at IS NULL ORDER BY id DESC LIMIT 1",
                [box_id.get()],
                session_from_row,
            )
            .optional()?)
    }

    /// Whether `box_id` has ever had a session, ended or not.
    pub(crate) fn has_session(&self, box_id: BoxId) -> Result<bool, StoreError> {
        let conn = self.reader()?;
        Ok(conn
            .query_row(
                "SELECT 1 FROM launches WHERE box = ? LIMIT 1",
                [box_id.get()],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Record the start of a session on `box_id` and its `SessionStarted` event; box state is not
    /// touched.
    pub(crate) fn begin_session(
        &self,
        box_id: BoxId,
        session_ref: &str,
        pane: Option<&str>,
    ) -> Result<i64, StoreError> {
        let now = self.now();
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if load_box(&tx, box_id)?.is_none() {
            return Err(StoreError::NotFound(box_id));
        }
        let current: Option<String> = tx
            .query_row(
                "SELECT state FROM sessions WHERE box = ?1 ORDER BY ordinal DESC LIMIT 1",
                [box_id.get()],
                |row| row.get(0),
            )
            .optional()?;
        if current.is_some_and(|state| state == "ended") {
            return Err(StoreError::Invalid(format!(
                "Box {} has no live Session to launch into",
                box_id.get()
            )));
        }
        tx.execute(
            "INSERT INTO launches (box, session_ref, pane, started_at, session) VALUES (?1, ?2, ?3, ?4, \
             (SELECT id FROM sessions WHERE box = ?1 ORDER BY ordinal DESC LIMIT 1))",
            params![box_id.get(), session_ref, pane, now.as_millis()],
        )?;
        let id = tx.last_insert_rowid();
        let started = Event::SessionStarted {
            session: session_ref.to_owned(),
        };
        insert_event(&tx, box_id, &started, now)?;
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
        let conn = self.writer();
        conn.execute(
            "UPDATE launches SET ended_at = ?, exit = ?, transcript_digest = COALESCE(?, transcript_digest) \
             WHERE id = ? AND ended_at IS NULL",
            params![now.as_millis(), exit, transcript_digest, id],
        )?;
        drop(conn);
        Ok(())
    }

    /// Keep the agent session id registered for resume against session `id`.
    pub(crate) fn set_registered_session(&self, id: i64, agent: &str) -> Result<(), StoreError> {
        let conn = self.writer();
        conn.execute(
            "UPDATE launches SET registered_agent_session = ? WHERE id = ?",
            params![agent, id],
        )?;
        drop(conn);
        Ok(())
    }

    /// Keep the digest of the transcript saved for a session that is still live.
    pub(crate) fn set_transcript_digest(&self, id: i64, digest: &str) -> Result<(), StoreError> {
        let conn = self.writer();
        conn.execute(
            "UPDATE launches SET transcript_digest = ? WHERE id = ? AND ended_at IS NULL",
            params![digest, id],
        )?;
        drop(conn);
        Ok(())
    }

    /// Cache `record` as the latest hook report of `box_id`, stamped with the read time.
    ///
    /// A record equal to the cached one keeps its reported mark.
    pub(crate) fn put_hook(&self, box_id: BoxId, record: &HookRecord) -> Result<(), StoreError> {
        let now = self.now();
        let json = serde_json::to_string(&record_json(record))?;
        let conn = self.writer();
        conn.execute(
            "INSERT INTO hook_cache (session, record, at) \
             VALUES ((SELECT id FROM sessions WHERE box = ?1 ORDER BY ordinal DESC LIMIT 1), ?2, ?3) \
             ON CONFLICT(session) DO UPDATE SET at = ?3, record = CASE \
             WHEN json_remove(hook_cache.record, '$.reported') = json(?2) THEN hook_cache.record \
             ELSE ?2 END",
            params![box_id.get(), json, now.as_millis()],
        )?;
        drop(conn);
        Ok(())
    }

    /// Mark the cached hook report of `box_id` written at `at` as reported by a `needs_input`
    /// event; a newer report left in its place stays unmarked.
    pub(crate) fn mark_hook_reported(&self, box_id: BoxId, at: Ts) -> Result<(), StoreError> {
        let conn = self.writer();
        conn.execute(
            "UPDATE hook_cache SET record = json_set(record, '$.reported', json('true')) \
             WHERE session = (SELECT id FROM sessions WHERE box = ?1 ORDER BY ordinal DESC LIMIT 1) \
             AND json_extract(record, '$.at') = ?2",
            params![box_id.get(), at.as_millis()],
        )?;
        drop(conn);
        Ok(())
    }

    /// The latest cached hook report of `box_id`.
    pub(crate) fn hook(&self, box_id: BoxId) -> Result<Option<HookCache>, StoreError> {
        let conn = self.reader()?;
        let row: Option<(String, i64)> = conn
            .query_row(
                "SELECT h.record, h.at FROM hook_cache h JOIN sessions s ON s.id = h.session \
                 WHERE s.box = ?1 AND s.ordinal = (SELECT MAX(ordinal) FROM sessions WHERE box = ?1)",
                [box_id.get()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        row.map(|(json, at)| {
            let value: Value = serde_json::from_str(&json)?;
            Ok(HookCache {
                record: record_from_json(&value)?,
                at: Ts::from_millis(at),
                reported: value
                    .get("reported")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .transpose()
    }
}

/// A session from a `launches` row.
fn session_from_row(r: &Row<'_>) -> rusqlite::Result<SessionRow> {
    Ok(SessionRow {
        id: r.get(0)?,
        box_id: BoxId::new(r.get(1)?),
        session_ref: r.get(2)?,
        pane: r.get(3)?,
        started_at: Ts::from_millis(r.get(4)?),
        ended_at: r.get::<_, Option<i64>>(5)?.map(Ts::from_millis),
        exit: r.get(6)?,
        transcript_digest: r.get(7)?,
        registered_agent_session: r.get(8)?,
        session: r.get(9)?,
    })
}

/// The stored form of a hook report.
fn record_json(rec: &HookRecord) -> Value {
    let kind = match &rec.kind {
        HookKind::Starting { .. } => "starting",
        HookKind::Busy => "busy",
        HookKind::Idle => "idle",
        HookKind::Blocked => "blocked",
    };
    json!({
        "kind": kind,
        "message": rec.message,
        "options": rec.options,
        "doing": rec.doing,
        "session_id": rec.session_id,
        "at": rec.at.as_millis(),
    })
}

/// A hook report from its stored form.
fn record_from_json(v: &Value) -> Result<HookRecord, StoreError> {
    let bad = |what: &str| StoreError::Invalid(format!("hook cache: {what}"));
    let kind = match v["kind"].as_str() {
        Some("starting") => HookKind::Starting { launch: None },
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
        options: v["options"]
            .as_array()
            .map(|o| {
                o.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        doing: v["doing"].as_str().map(str::to_owned),
        at: Ts::from_millis(v["at"].as_i64().ok_or_else(|| bad("missing at"))?),
    })
}

#[cfg(test)]
mod tests;
