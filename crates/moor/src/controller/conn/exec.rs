//! The `exec` request: runs a command in a Box with its stdio streamed over the connection.
//!
//! The client's bytes frames feed the command's stdin and its eof frame closes it; the
//! command's stdout and stderr come back as bytes and stderr frames. The client keeps its
//! sending side open until the answer: a closed side, or the controller's drain, kills the
//! command.

use std::io::{self, BufRead, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Mutex, PoisonError};
use std::thread;

use moor_core::controller::{parse_frame, write_bytes, write_stderr, ClientFrame, BYTES_MAX};
use moor_core::request::Grant;
use moor_core::seam::proc::Pipes;
use moor_core::view::ExecView;
use moor_core::{ErrorKind, ExecReq, Response};

use super::{Duplex, Peer};
use crate::env::Env;
use crate::error::CommandError;
use crate::Moor;

/// Stdin chunks held for a command that is not reading; the client waits past this many.
const STDIN_QUEUE: usize = 16;

/// Run the command `req` names over this connection, then answer with how it ended.
pub(super) fn exec(
    env: &Env,
    peer: &Peer,
    grant: Grant,
    id: &str,
    req: &ExecReq,
    (input, output): Duplex<'_>,
) -> Response {
    let refused = |e: CommandError| e.into_response("exec", vec![]);
    let Some(hangup) = peer.hangup.as_ref() else {
        return refused(CommandError::new(
            ErrorKind::Failed,
            "this connection cannot stream a command",
        ));
    };
    let moor = match Moor::from_env(env) {
        Ok(moor) => moor.granted(grant),
        Err(e) => return refused(e),
    };
    let drain = peer.conn.drain();
    let link = Link {
        id,
        input,
        output,
        unblock: &|| hangup.stop_reading(),
        stopping: &|| drain.begun(),
    };
    answer(stream(link, |pipes| moor.exec(req, pipes)))
}

/// The answer to an `exec` that ended as `ended`.
pub(crate) fn answer(ended: Result<ExecView, CommandError>) -> Response {
    match ended {
        Ok(view) => Response::ok("exec", serde_json::to_value(view).ok()),
        Err(e) => e.into_response("exec", vec![]),
    }
}

/// The connection one `exec` streams over.
pub(crate) struct Link<'a> {
    /// Id of the `exec` request; frames for another id end it.
    pub(crate) id: &'a str,
    /// The client's frames.
    pub(crate) input: &'a mut (dyn BufRead + Send),
    /// Where the command's output goes, as frames.
    pub(crate) output: &'a mut (dyn Write + Send),
    /// Wakes a read blocked on the client's frames once the command is over.
    pub(crate) unblock: &'a (dyn Fn() + Sync),
    /// True once the controller began to restart.
    pub(crate) stopping: &'a (dyn Fn() -> bool + Sync),
}

/// Runs `run` with pipes wired to `link` and returns what it returns.
pub(crate) fn stream<T>(link: Link<'_>, run: impl FnOnce(Pipes<'_>) -> T) -> T {
    let Link {
        id,
        input,
        output,
        unblock,
        stopping,
    } = link;
    let (feed, queue) = mpsc::sync_channel(STDIN_QUEUE);
    let gone = AtomicBool::new(false);
    let output = Mutex::new(output);
    thread::scope(|s| {
        s.spawn(|| {
            take_input(id, input, feed);
            gone.store(true, Ordering::SeqCst);
        });
        let mut stdout = Frames {
            id,
            output: &output,
            write: write_bytes,
        };
        let mut stderr = Frames {
            id,
            output: &output,
            write: write_stderr,
        };
        let stop = || gone.load(Ordering::SeqCst) || stopping();
        let ended = run(Pipes {
            stdin: Box::new(Queued::new(queue)),
            stdout: &mut stdout,
            stderr: &mut stderr,
            stop: &stop,
        });
        unblock();
        ended
    })
}

/// Feeds the client's bytes frames to the command's stdin until an eof frame, then reads on
/// until the client shuts its side or sends anything else; resize frames are ignored.
fn take_input(id: &str, input: &mut (dyn BufRead + Send), feed: SyncSender<Vec<u8>>) {
    let mut feed = Some(feed);
    let mut line = String::new();
    loop {
        line.clear();
        if !matches!(input.read_line(&mut line), Ok(n) if n > 0) {
            return;
        }
        match parse_frame(line.trim_end()) {
            Ok(ClientFrame::Bytes { id: got, len }) if got == id && len <= BYTES_MAX => {
                let mut buf = vec![0u8; len];
                if input.read_exact(&mut buf).is_err() {
                    return;
                }
                // A command that stopped reading its stdin drops what follows.
                if feed.as_ref().is_some_and(|f| f.send(buf).is_err()) {
                    feed = None;
                }
            }
            Ok(ClientFrame::Eof { id: got }) if got == id => feed = None,
            Ok(ClientFrame::Resize { id: got, .. }) if got == id => {}
            _ => return,
        }
    }
}

/// One of the command's output streams, written as frames of one kind to the shared output.
struct Frames<'a, 'o> {
    /// Id of the `exec` request.
    id: &'a str,
    /// The client's side of the connection, shared by stdout and stderr.
    output: &'a Mutex<&'o mut (dyn Write + Send)>,
    /// Writes one chunk as frames of this stream's kind.
    write: fn(&mut dyn Write, &str, &[u8]) -> io::Result<()>,
}

impl Write for Frames<'_, '_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut out = self.output.lock().unwrap_or_else(PoisonError::into_inner);
        (self.write)(&mut **out, self.id, buf).map(|()| buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The command's stdin: the chunks the client sent, ending when the client's eof frame or
/// its going away drops the feed.
struct Queued {
    /// Chunks still to come.
    queue: Receiver<Vec<u8>>,
    /// The chunk being read and how far.
    chunk: io::Cursor<Vec<u8>>,
}

impl Queued {
    /// Reads the chunks of `queue`.
    const fn new(queue: Receiver<Vec<u8>>) -> Self {
        Self {
            queue,
            chunk: io::Cursor::new(Vec::new()),
        }
    }
}

impl Read for Queued {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let n = self.chunk.read(buf)?;
            if n > 0 || buf.is_empty() {
                return Ok(n);
            }
            match self.queue.recv() {
                Ok(chunk) => self.chunk = io::Cursor::new(chunk),
                Err(_) => return Ok(0),
            }
        }
    }
}

#[cfg(test)]
mod tests;
