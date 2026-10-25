//! Stopping a Box from the Stop button on its status message. Only the person who asked for
//! the Box may press it. The Socket Mode thread records the click and queues the stop; the
//! worker sends the interrupt to the Box as its requester.

use moor_core::name::BoxName;
use moor_core::request::{Request, SteerReq};
use moor_core::response::ExitClass;

use super::{error_text, Fx, Job};
use crate::db::{BoxState, Db};
use crate::escape::escape;
use crate::socket::Click;

/// What a press of Stop on the status message of Box `name` does: queue the interrupt for the
/// Box's requester, refuse anyone else, and say so when the Box is closing or closed. Nothing
/// when the message is not that Box's status message.
pub(super) fn click(click: &Click, name: &str, db: &Db) -> rusqlite::Result<Option<Job>> {
    let Some(boxed) = db.box_by_status(&click.channel, &click.message_ts)? else {
        return Ok(None);
    };
    if boxed.box_name.as_str() != name {
        return Ok(None);
    }
    let tell = |text: String| Job::Ephemeral {
        channel: boxed.channel.clone(),
        user: click.user.clone(),
        thread: boxed.thread_ts.clone(),
        text,
    };
    let by = &boxed.requested_by;
    if &click.user != by {
        return Ok(Some(tell(format!("Only <@{by}> can stop this Box."))));
    }
    Ok(Some(match boxed.state {
        BoxState::Bound { .. } => Job::Stop {
            name: boxed.box_name.clone(),
            channel: boxed.channel.clone(),
            thread: boxed.thread_ts.clone(),
            by: by.clone(),
        },
        _ => tell(format!("Box `{name}` is closing or closed.")),
    }))
}

/// Interrupts the agent of Box `name` as its requester `by`. A refusal or a failure is posted
/// in the thread; a Box that takes it shows its new state in its status message.
pub(super) fn stop(fx: &mut Fx<'_>, name: &BoxName, (channel, thread): (&str, &str), by: &str) {
    let steer = Request::Steer(SteerReq {
        target: name.clone().into(),
        text: None,
        menu: None,
        interrupt: true,
    });
    let said = match (fx.call)(by, steer) {
        Ok(reply) if reply.class == ExitClass::Ok => return,
        Ok(reply) => format!(
            "Box `{name}` refused the interrupt: {}",
            escape(&error_text(reply)),
            name = name.as_str()
        ),
        Err(e) => format!(
            "The interrupt may not have reached Box `{}`: {}",
            name.as_str(),
            escape(&e.to_string())
        ),
    };
    (fx.reply)(channel, thread, &said);
}
