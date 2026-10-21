//! `moor events` without a controller: the local store streamed in-process.

use std::io::{self, Write};

use moor_core::controller::{parse_server_frame, ServerFrame, WatchFilter};
use moor_core::user::Viewer;

use super::watch;
use crate::env::Env;

/// Stream this machine's events to `out`, one JSON frame per line.
///
/// # Errors
/// `kind: message` when the stream ends with an error frame, which is not printed.
pub fn watch_local(
    env: &Env,
    since: i64,
    filter: &WatchFilter,
    follow: bool,
    out: &mut impl Write,
) -> Result<(), String> {
    let mut sink = Sink { out, failure: None };
    let ask = watch::Ask {
        since,
        filter: filter.clone(),
        follow,
        viewer: Viewer::Operator,
    };
    watch::stream(&mut sink, env, ask, watch::Limits::default());
    sink.failure.map_or(Ok(()), Err)
}

/// Forwards frame lines to `out` and keeps an error frame instead of forwarding it.
struct Sink<'a, W: Write> {
    /// Where frames go.
    out: &'a mut W,
    /// The error frame seen, as `kind: message`.
    failure: Option<String>,
}

impl<W: Write> Write for Sink<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let line = String::from_utf8_lossy(buf);
        if let Ok(ServerFrame::Error { kind, message, .. }) = parse_server_frame(line.trim_end()) {
            self.failure = Some(format!("{kind:?}: {message}"));
        } else {
            self.out.write_all(buf)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

#[cfg(test)]
mod tests;
