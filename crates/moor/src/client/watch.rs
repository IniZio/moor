//! `moor events`: the controller's event stream copied to a writer, one JSON frame per line.

use std::io::{BufRead, Write};

use moor_client::Attempt;
use moor_core::controller::{ClientFrame, FrameErrorKind, ServerFrame, WatchFilter};

use super::{
    handshake, parse_frame, redialing, send, with_transport, ClientError, SessionError, Target,
};
use crate::env::Env;

/// Stream events from the controller at `target` to `out`, one JSON frame per line. A stream
/// the controller ends with `restarting` resumes after the last cursor it delivered.
///
/// # Errors
/// [`ClientError::Unreachable`] when the transport cannot be opened, [`ClientError::Protocol`]
/// when the controller breaks the protocol or sends an error frame. A failing `out` ends the
/// stream quietly.
pub fn watch(
    env: &Env,
    target: &Target,
    since: i64,
    filter: &WatchFilter,
    follow: bool,
    out: &mut impl Write,
) -> Result<(), ClientError> {
    let mut since = since;
    let mut restarted = None;
    loop {
        let mut first = restarted.take().map(ClientError::Protocol);
        let ended = redialing(
            env,
            target,
            |_| {},
            || {
                first.take().map_or_else(
                    || {
                        with_transport(env, target, false, |reader, mut writer| {
                            watch_session(reader, &mut writer, &mut since, filter, follow, out)
                        })
                    },
                    Attempt::Unrun,
                )
            },
        )?;
        if ended.is_none() {
            return Ok(());
        }
        restarted = ended;
    }
}

/// The hello, watch request and frame copy over an open transport. Moves `since` past each
/// delivered cursor; `Some` is the message of a `restarting` end.
fn watch_session(
    mut reader: impl BufRead,
    writer: &mut impl Write,
    since: &mut i64,
    filter: &WatchFilter,
    follow: bool,
    out: &mut impl Write,
) -> Result<Option<String>, SessionError> {
    handshake(&mut reader, writer)?;
    send(
        writer,
        &ClientFrame::Watch {
            since: *since,
            boxes: filter.boxes.clone(),
            kinds: filter.kinds.clone(),
            parent: filter.parent,
            follow,
            identity: None,
        },
    )
    .map_err(ClientError::Protocol)?;
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .map_err(|e| ClientError::Protocol(format!("read from controller: {e}")))?;
        if n == 0 {
            return Ok(None);
        }
        let frame = parse_frame(&line)?;
        match &frame {
            ServerFrame::Error {
                kind: FrameErrorKind::Restarting,
                message,
                ..
            } => return Ok(Some(message.clone())),
            ServerFrame::Error { kind, message, .. } => {
                return Err(ClientError::Protocol(format!("{kind:?}: {message}")).into())
            }
            ServerFrame::Event { cursor, .. } => *since = *cursor,
            ServerFrame::CaughtUp { cursor } => *since = (*since).max(*cursor),
            _ => {}
        }
        if out
            .write_all(frame.to_line().as_bytes())
            .and_then(|()| out.flush())
            .is_err()
        {
            return Ok(None);
        }
    }
}

#[cfg(test)]
mod tests;
