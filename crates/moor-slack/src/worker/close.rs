//! The close path: a Box marked `closing` is closed over the controller socket, asked about
//! again after a lost connection, and its result or the controller's refusal posted.

use moor_core::request::{CloseReq, Request};
use moor_core::response::{CloseRefusal, ErrorDetail, ExitClass, Response};
use serde_json::Value;

use super::{error_text, lost, show_request, written, Attempt, Fx};
use crate::command::{unsaved_work, BoxEvent};
use crate::db::{BoxState, Db, SlackBox};

/// How one attempt at a close ends.
enum Closed {
    /// The Box is closed, or the controller has no such Box.
    Done,
    /// The Box stays open because it holds work that is not pushed.
    Unsaved {
        /// Unpushed commits.
        ahead: u32,
        /// Uncommitted files.
        dirty: u32,
    },
    /// The Box stays open because the controller could not check it for unsaved work.
    Unchecked(String),
    /// The Box stays open; the text says why.
    Refused(String),
    /// A close may have run and the controller cannot confirm it now; the text says why.
    Unsure(String),
}

/// Tries once to settle the close of `row`: posts the result and records it, or logs why the
/// Box stays `closing`. Returns whether the close is settled.
pub(super) fn settle_close(db: &Db, fx: &mut Fx<'_>, row: &SlackBox, attempt: Attempt) -> bool {
    let name = row.box_name.as_str();
    let (text, event) = match ask(fx, row, attempt) {
        Closed::Done => (format!("Closed Box `{name}`."), BoxEvent::Closed),
        Closed::Unsaved { ahead, dirty } => (
            format!(
                "Box `{name}` is still open: it holds work that is not saved anywhere ({}). \
                 Closing it would discard that work, so nothing is lost unless you confirm. \
                 If you still want it closed, say so in this thread.",
                unsaved_work(ahead, dirty)
            ),
            BoxEvent::Unsaved { ahead, dirty },
        ),
        Closed::Unchecked(reason) => {
            (fx.log)(&format!("close of {name}: Box not checked: {reason}"));
            (
                format!(
                    "Box `{name}` is still open: I could not check it for work that is not saved \
                     anywhere, and closing it could discard that work. If you want it closed \
                     anyway, ask me to close it and discard its work."
                ),
                BoxEvent::CloseRefused,
            )
        }
        Closed::Refused(message) => (
            format!("Could not close Box `{name}`: {message}"),
            BoxEvent::CloseRefused,
        ),
        Closed::Unsure(why) => {
            (fx.log)(&format!("close of {name}: {why}; asking again"));
            return false;
        }
    };
    (fx.reply)(&row.channel, &row.thread_ts, &text);
    let gone = event == BoxEvent::Closed;
    written(
        fx,
        db.atomically(|| {
            db.settle_close(&row.box_name, gone)?;
            // A Box closes once, but each close the starter asks for anew may be refused.
            let cursor = if gone { None } else { Some(db.next_item_id()?) };
            db.notice(row, event, cursor)?;
            Ok(())
        }),
    )
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
    match (fx.call)(&row.requested_by, show_request(row)) {
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
        force: matches!(row.state, BoxState::Closing { discard: true, .. }),
    });
    match (fx.call)(&row.requested_by, request) {
        Ok(reply) if reply.class == ExitClass::Ok => Closed::Done,
        Ok(reply) => refusal(reply),
        Err(e) if matches!(attempt, Attempt::First) && !lost(&e) => Closed::Refused(e.to_string()),
        Err(e) => Closed::Unsure(e.to_string()),
    }
}

/// How a refused close ends, read from the refusal's kind and never from its text.
fn refusal(reply: Response) -> Closed {
    let kind = reply
        .error
        .as_ref()
        .and_then(|e| e.detail.as_ref())
        .map(|ErrorDetail::CloseRefused(kind)| kind.clone());
    match kind {
        Some(CloseRefusal::AlreadyClosed) => Closed::Done,
        Some(CloseRefusal::Unsaved { ahead, dirty, .. }) => Closed::Unsaved { ahead, dirty },
        Some(CloseRefusal::Unchecked { reason }) => Closed::Unchecked(reason),
        None => Closed::Refused(error_text(reply)),
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
