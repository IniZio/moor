//! `moor events`: the controller's event stream copied to a writer, one JSON frame per line.

use std::io::{BufRead, Write};

use moor_core::controller::{ClientFrame, ServerFrame, WatchFilter};

use super::{handshake, parse_frame, send, with_transport, ClientError, SessionError, Target};
use crate::env::Env;

/// Stream events from the controller at `target` to `out`, one JSON frame per line.
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
    with_transport(env, target, |reader, mut writer| {
        watch_session(reader, &mut writer, since, filter, follow, out)
    })
}

/// The hello, watch request and frame copy over an open transport.
fn watch_session(
    mut reader: impl BufRead,
    writer: &mut impl Write,
    since: i64,
    filter: &WatchFilter,
    follow: bool,
    out: &mut impl Write,
) -> Result<(), SessionError> {
    handshake(&mut reader, writer)?;
    send(
        writer,
        &ClientFrame::Watch {
            since,
            boxes: filter.boxes.clone(),
            kinds: filter.kinds.clone(),
            follow,
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
            return Ok(());
        }
        let frame = parse_frame(&line)?;
        if let ServerFrame::Error { kind, message, .. } = &frame {
            return Err(ClientError::Protocol(format!("{kind:?}: {message}")).into());
        }
        if out
            .write_all(frame.to_line().as_bytes())
            .and_then(|()| out.flush())
            .is_err()
        {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests;
