//! Events the fake controller stores and sends to the watches that carry them.

use std::io::Write as _;

use moor_core::controller::ServerFrame;
use moor_core::name::BoxId;
use serde_json::Value;

use super::{carries, FakeController, BACKLOG_END};

impl FakeController {
    /// Stores an event of `kind` for Box `box_id` named `name`, and sends it to every live
    /// watch.
    pub(crate) fn emit(&self, box_id: i64, name: &str, kind: &str, payload: Value) {
        self.emit_owned(None, (box_id, name, kind), payload, now_millis());
    }

    /// Like [`Self::emit`], stamped `ago_secs` seconds in the past.
    pub(crate) fn emit_ago(&self, ago_secs: i64, ids: (i64, &str, &str), payload: Value) {
        self.emit_owned(None, ids, payload, now_millis() - ago_secs * 1000);
    }

    /// Like [`Self::emit`], for a Box that `owner` owns: only a watch asserting `owner` carries it.
    pub(crate) fn emit_for(
        &self,
        owner: &str,
        box_id: i64,
        name: &str,
        kind: &str,
        payload: Value,
    ) {
        self.emit_owned(Some(owner), (box_id, name, kind), payload, now_millis());
    }

    /// Stores the event, owned by `owner` when given, and sends it to the watches that carry it.
    fn emit_owned(
        &self,
        owner: Option<&str>,
        (box_id, name, kind): (i64, &str, &str),
        payload: Value,
        ts: i64,
    ) {
        let mut inner = self.lock();
        let cursor = BACKLOG_END + 1 + i64::try_from(inner.events.len()).unwrap();
        let event = ServerFrame::Event {
            cursor,
            r#box: Some(name.to_owned()),
            box_id: Some(BoxId::new(box_id)),
            session: None,
            kind: kind.to_owned(),
            payload,
            ts,
        };
        let line = event.to_line();
        let owner = owner.map(str::to_owned);
        inner.events.push((owner.clone(), event));
        inner.live.retain_mut(|(watcher, stream)| {
            !carries(owner.as_deref(), watcher.as_deref())
                || stream.write_all(line.as_bytes()).is_ok()
        });
    }
}

/// The wall clock in milliseconds, which a real controller stamps its events with.
#[allow(
    clippy::disallowed_methods,
    reason = "the fake controller stamps events like a real one, with the wall clock"
)]
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}
