//! The idle sweep: a thread quiet for the configured minutes, with no Box of its own at work
//! or waiting on a question, gets its host Session ended. Its Boxes are never touched; the
//! next message opens a new host Session in the same workdir.

use moor_core::name::SessionId;
use moor_core::request::{EndHostSessionReq, Request, ShowReq};
use moor_core::response::{ExitClass, Response};
use moor_core::ErrorKind;
use serde_json::Value;

use super::Turns;
use crate::db::{BoxState, Db};

/// Agent states, as `show` names them, that keep a thread's host Session open.
const BUSY: [&str; 4] = ["starting", "working", "needs_input", "unknown"];

impl Turns {
    /// Ends the host Session of every thread quiet since `idle_secs` ago.
    pub(super) fn sweep(&self, db: &Db) {
        let cutoff = (self.clock)() - self.idle_secs;
        let quiet = match db.quiet_threads(cutoff) {
            Ok(quiet) => quiet,
            Err(e) => return (self.log)(&format!("idle: slack.db: {e}")),
        };
        for (channel, thread_ts, session) in quiet {
            if let Some(why) = self.busy_box(db, &channel, &thread_ts) {
                (self.log)(&format!(
                    "idle: thread {channel}/{thread_ts} stays open: {why}"
                ));
                continue;
            }
            self.end_host(db, &channel, &thread_ts, session);
        }
    }

    /// Ends host Session `session` of the thread and forgets it once the controller says it
    /// has ended. Any other answer leaves the record for a later try.
    pub(super) fn end_host(&self, db: &Db, channel: &str, thread_ts: &str, session: i64) {
        let thread = format!("{channel}/{thread_ts}");
        let end = Request::EndHostSession(EndHostSessionReq {
            session: SessionId::new(session),
        });
        match self.client.request(end, |_| {}) {
            Ok(reply) if ended(&reply) => {
                self.written(db.clear_root_session(channel, thread_ts, session));
                (self.log)(&format!(
                    "host: thread {thread} ended host Session {session}"
                ));
            }
            Ok(reply) => (self.log)(&format!("host: thread {thread}: {}", text(&reply))),
            Err(e) => (self.log)(&format!("host: thread {thread}: {e}")),
        }
    }

    /// Why a running Box of the thread keeps it open: its agent is busy, or `show` did not
    /// say. `None` when no running Box is busy. A Box the controller has no record of is
    /// marked gone and keeps nothing open.
    fn busy_box(&self, db: &Db, channel: &str, thread_ts: &str) -> Option<String> {
        let boxes = match db.thread_boxes(channel, thread_ts) {
            Ok(boxes) => boxes,
            Err(e) => return Some(format!("slack.db: {e}")),
        };
        boxes
            .iter()
            .filter(|b| matches!(b.state, BoxState::Bound { .. }))
            .find_map(|b| {
                let name = b.box_name.as_str();
                let show = Request::Show(ShowReq {
                    target: Some(b.box_name.clone().into()),
                    events: None,
                    agent: true,
                    reports: false,
                    artifacts: false,
                    pane: false,
                });
                let reply = match self.client.request(show, |_| {}) {
                    Ok(reply) if reply.class == ExitClass::Ok => reply,
                    Ok(reply) if text(&reply) == format!("no Box {name}") => {
                        self.written(db.mark_gone(&b.box_name));
                        (self.log)(&format!("idle: Box {name} is gone"));
                        return None;
                    }
                    Ok(reply) => return Some(format!("show {name}: {}", text(&reply))),
                    Err(e) => return Some(format!("show {name}: {e}")),
                };
                let view = reply.value.unwrap_or(Value::Null);
                let state = view.pointer("/box/agent/state").and_then(Value::as_str)?;
                BUSY.contains(&state)
                    .then(|| format!("Box {name} is {state}"))
            })
    }
}

/// True when the end took, or the controller refused it because the Session had already
/// ended or is unknown to it.
fn ended(reply: &Response) -> bool {
    reply.class == ExitClass::Ok
        || reply
            .error
            .as_ref()
            .is_some_and(|e| e.kind == ErrorKind::Refused)
}

/// The error text of a failed reply.
fn text(reply: &Response) -> String {
    reply
        .error
        .as_ref()
        .map_or_else(|| format!("{} failed", reply.cmd), |e| e.message.clone())
}
