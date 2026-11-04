//! Agent session rows and the last hook report cached per box.

use moor_core::activity::{HookCache, HookKind, HookRecord};
use moor_core::config::AgentHolder;
use moor_core::domain::event::Event;
use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use serde_json::{json, Value};

use moor_schema::tables;
use sea_query::{Expr, ExprTrait as _, OnConflict, Query};

use super::apply::insert_event;
use super::select::newest_session;
use super::{load_box, Store, StoreError};
use crate::store::db::{self, params, Db, DbExt as _, OptionalExtension, Row, TransactionBehavior};

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
    /// What holds the agent's terminal on the Box.
    pub(crate) holder: AgentHolder,
}

impl Store {
    /// The newest session of `box_id` that has not ended.
    pub(crate) fn live_session(&self, box_id: BoxId) -> Result<Option<SessionRow>, StoreError> {
        let conn = self.reader()?;
        Ok(conn
            .query_row(
                "SELECT id, box, session_ref, pane, started_at, ended_at, exit, transcript_digest, \
                 registered_agent_session, session, holder FROM launches WHERE box = ? AND ended_at IS NULL ORDER BY id DESC LIMIT 1",
                [box_id.get()],
                session_from_row,
            )
            .optional()?)
    }

    /// What holds the terminal of the newest agent of `box_id`, ended or not.
    pub(crate) fn agent_holder(&self, box_id: BoxId) -> Result<AgentHolder, StoreError> {
        let conn = self.reader()?;
        let stored: Option<Option<String>> = conn
            .query_row(
                "SELECT holder FROM launches WHERE box = ? ORDER BY id DESC LIMIT 1",
                [box_id.get()],
                |r| r.get(0),
            )
            .optional()?;
        holder_from(stored.flatten())
            .map_err(|e| StoreError::Corrupt(format!("launches.holder: {e}")))
    }

    /// Forget the holders of `box_id`'s sessions, as moor did before it recorded them.
    #[cfg(test)]
    pub(crate) fn forget_holders(&self, box_id: BoxId) -> Result<(), StoreError> {
        let conn = self.writer();
        conn.execute(
            "UPDATE launches SET holder = NULL WHERE box = ?",
            [box_id.get()],
        )?;
        drop(conn);
        Ok(())
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

    /// Record the start of a session on `box_id`, held by `holder`, and its `SessionStarted`
    /// event; box state is not touched.
    pub(crate) fn begin_session(
        &self,
        box_id: BoxId,
        session_ref: &str,
        pane: Option<&str>,
        holder: AgentHolder,
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
        let id: i64 = tx.query_row(
            "INSERT INTO launches (box, session_ref, pane, started_at, session, holder) VALUES (?1, ?2, ?3, ?4, \
             (SELECT id FROM sessions WHERE box = ?1 ORDER BY ordinal DESC LIMIT 1), ?5) RETURNING id",
            params![box_id.get(), session_ref, pane, now.as_millis(), holder.name()],
            |r| r.get(0),
        )?;
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
        let record = record_json(record);
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let unchanged = cached_hook(&tx, box_id)?.is_some_and(|mut cached| {
            if let Some(fields) = cached.as_object_mut() {
                fields.shift_remove("reported");
            }
            cached == record
        });
        let mut on_conflict = OnConflict::column(tables::HookCache::Session);
        on_conflict.update_column(tables::HookCache::At);
        if !unchanged {
            on_conflict.update_column(tables::HookCache::Record);
        }
        tx.run(
            Query::insert()
                .into_table(tables::HookCache::Table)
                .columns([
                    tables::HookCache::Session,
                    tables::HookCache::Record,
                    tables::HookCache::At,
                ])
                .values_panic([
                    db::sub(newest_session(box_id)),
                    serde_json::to_string(&record)?.into(),
                    now.as_millis().into(),
                ])
                .on_conflict(on_conflict),
        )?;
        tx.commit()?;
        drop(conn);
        Ok(())
    }

    /// Mark the cached hook report of `box_id` written at `at` as reported by a `needs_input`
    /// event; a newer report left in its place stays unmarked.
    pub(crate) fn mark_hook_reported(&self, box_id: BoxId, at: Ts) -> Result<(), StoreError> {
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(mut record) = cached_hook(&tx, box_id)? else {
            return Ok(());
        };
        let Some(fields) = record
            .as_object_mut()
            .filter(|f| f.get("at").and_then(Value::as_i64) == Some(at.as_millis()))
        else {
            return Ok(());
        };
        fields.insert("reported".to_owned(), Value::Bool(true));
        tx.run(
            Query::update()
                .table(tables::HookCache::Table)
                .value(tables::HookCache::Record, serde_json::to_string(&record)?)
                .and_where(
                    Expr::col(tables::HookCache::Session).eq(db::sub(newest_session(box_id))),
                ),
        )?;
        tx.commit()?;
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
        holder: holder_from(r.get(10)?).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(10, rusqlite::types::Type::Text, e.into())
        })?,
    })
}

/// The holder a `launches.holder` value names. A launch from before moor recorded the holder
/// has none, and was held by [`AgentHolder::UNRECORDED`].
fn holder_from(stored: Option<String>) -> Result<AgentHolder, String> {
    stored.map_or(Ok(AgentHolder::UNRECORDED), |name| {
        AgentHolder::from_name(&name).ok_or_else(|| format!("unknown holder {name:?}"))
    })
}

/// The hook report cached for Box `box_id`'s newest Session, as stored.
///
/// Its fields are read and written here rather than in SQL, so no database's JSON functions
/// are needed.
fn cached_hook(db: &dyn Db, box_id: BoxId) -> Result<Option<Value>, StoreError> {
    let q = Query::select()
        .column(tables::HookCache::Record)
        .from(tables::HookCache::Table)
        .and_where(Expr::col(tables::HookCache::Session).eq(db::sub(newest_session(box_id))))
        .take();
    let Some(raw) = db.row(q, |r| r.get::<_, String>(0)).optional()? else {
        return Ok(None);
    };
    Ok(Some(serde_json::from_str(&raw)?))
}

/// The stored form of a hook report.
fn record_json(rec: &HookRecord) -> Value {
    let on = match &rec.kind {
        HookKind::Waiting { on } => on.as_slice(),
        _ => &[],
    };
    let kind = match &rec.kind {
        HookKind::Starting { .. } => "starting",
        HookKind::Busy => "busy",
        HookKind::Idle => "idle",
        HookKind::Blocked => "blocked",
        HookKind::Waiting { .. } => "waiting",
    };
    json!({
        "kind": kind,
        "message": rec.message,
        "options": rec.options,
        "on": on,
        "doing": rec.doing,
        "session_id": rec.session_id,
        "at": rec.at.as_millis(),
        "turn": rec.turn.map(Ts::as_millis),
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
        Some("waiting") => HookKind::Waiting {
            on: v["on"]
                .as_array()
                .map(|o| {
                    o.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
        },
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
        turn: v["turn"].as_i64().map(Ts::from_millis),
    })
}

#[cfg(test)]
mod tests;
