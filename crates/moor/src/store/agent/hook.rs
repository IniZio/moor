//! The last hook report cached per Box.

use moor_core::activity::{HookCache, HookKind, HookRecord};
use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use moor_schema::tables;
use sea_query::{Expr, ExprTrait as _, OnConflict, Query};
use serde_json::{json, Value};

use crate::store::db::{self, Db, DbExt as _, OptionalExtension};
use crate::store::select::newest_session;
use crate::store::{Store, StoreError};

impl Store {
    /// Cache `record` as the latest hook report of `box_id`, stamped with the read time.
    ///
    /// A record equal to the cached one keeps its reported mark.
    pub(crate) fn put_hook(&self, box_id: BoxId, record: &HookRecord) -> Result<(), StoreError> {
        let now = self.now();
        let record = record_json(record);
        let mut conn = self.writer();
        let tx = conn.immediate()?;
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
        let tx = conn.immediate()?;
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
            .row(
                Query::select()
                    .columns([tables::HookCache::Record, tables::HookCache::At])
                    .from(tables::HookCache::Table)
                    .and_where(
                        Expr::col(tables::HookCache::Session).eq(db::sub(newest_session(box_id))),
                    ),
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
