//! The close path: a Box marked `closing` is closed over the controller socket, asked about
//! again after a lost connection, and its result or the controller's refusal posted.

use moor_core::request::{CloseReq, Request};
use moor_core::response::{ExitClass, Response};
use serde_json::Value;

use super::{error_text, lost, show_request, written, Attempt, Fx};
use crate::db::{Db, SlackBox};

/// The controller's refusal of a close for a Box that is already closed.
const ALREADY_CLOSED: &str = "Box is already closed";

/// How one attempt at a close ends.
enum Closed {
    /// The Box is closed, or the controller has no such Box.
    Done,
    /// The Box stays open; the text says why.
    Refused(String),
    /// A close may have run and the controller cannot confirm it now; the text says why.
    Unsure(String),
}

/// Tries once to settle the close of `row`: posts the result and records it, or logs why the
/// Box stays `closing`. Returns whether the close is settled.
pub(super) fn settle_close(db: &Db, fx: &mut Fx<'_>, row: &SlackBox, attempt: Attempt) -> bool {
    let name = row.box_name.as_str();
    let (text, gone) = match ask(fx, row, attempt) {
        Closed::Done => (format!("Closed Box `{name}`."), true),
        Closed::Refused(message) => (format!("Could not close Box `{name}`: {message}"), false),
        Closed::Unsure(why) => {
            (fx.log)(&format!("close of {name}: {why}; asking again"));
            return false;
        }
    };
    (fx.reply)(&row.channel, &row.thread_ts, &text);
    written(fx, db.settle_close(&row.box_name, gone))
}

/// Sends at most one `show` and one `close` for `row`. After a lost attempt `show` goes
/// first, because a close that ran closes the Box and a second one is refused.
fn ask(fx: &mut Fx<'_>, row: &SlackBox, attempt: Attempt) -> Closed {
    if matches!(attempt, Attempt::Again) {
        if let Some(settled) = look(fx, row) {
            return settled;
        }
    }
    send(fx, row, attempt)
}

/// What a `show` says after a lost attempt; `None` when the Box is still open.
fn look(fx: &mut Fx<'_>, row: &SlackBox) -> Option<Closed> {
    match (fx.call)(show_request(row)) {
        Ok(reply) if is_closed(&reply, row) => Some(Closed::Done),
        Ok(reply) if reply.class != ExitClass::Ok => Some(Closed::Unsure(error_text(reply))),
        Ok(_) => None,
        Err(e) => Some(Closed::Unsure(e.to_string())),
    }
}

/// Sends one `close` for `row`.
fn send(fx: &mut Fx<'_>, row: &SlackBox, attempt: Attempt) -> Closed {
    let request = Request::Close(CloseReq {
        target: row.box_name.clone().into(),
        force: false,
        trust_moor_yaml: false,
    });
    match (fx.call)(request) {
        Ok(reply) if reply.class == ExitClass::Ok => Closed::Done,
        Ok(reply) => match error_text(reply) {
            message if message == ALREADY_CLOSED => Closed::Done,
            message => Closed::Refused(message),
        },
        Err(e) if matches!(attempt, Attempt::First) && !lost(&e) => Closed::Refused(e.to_string()),
        Err(e) => Closed::Unsure(e.to_string()),
    }
}

/// True when a `show` reply says the Box is closed or that the controller has no such Box.
fn is_closed(reply: &Response, row: &SlackBox) -> bool {
    if reply.class != ExitClass::Ok {
        let absent = format!("no Box {}", row.box_name.as_str());
        return reply.error.as_ref().is_some_and(|e| e.message == absent);
    }
    let lifecycle = reply
        .value
        .as_ref()
        .and_then(|v| v.get("box"))
        .and_then(|b| b.get("lifecycle"))
        .and_then(Value::as_str);
    lifecycle == Some("closed")
}
