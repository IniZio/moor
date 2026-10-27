//! The `attach` relay: runs a command under a pty and relays it both ways over one connection.
//!
//! The pty's output goes to the client as bytes frames. The client's bytes frames go to the pty
//! as keystrokes and its resize frames resize the pty. The relay ends when the client shuts its
//! side, the command exits, or the controller begins to restart.

use std::io::{self, BufRead, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use moor_core::controller::{parse_frame, write_bytes, ClientFrame, BYTES_MAX};
use moor_core::view::{AttachEnd, AttachView};
use portable_pty::{
    native_pty_system, ChildKiller, CommandBuilder, ExitStatus, MasterPty, PtySize,
};

/// How often the relay checks whether the controller began to restart.
const STOP_POLL: Duration = Duration::from_millis(100);

/// The terminal type the relayed command sees; the controller itself runs without one.
const TERM: &str = "xterm-256color";

/// The connection one relay runs over.
pub(crate) struct Link<'a> {
    /// Id of the `attach` request; frames for another id end the relay.
    pub(crate) id: &'a str,
    /// The client's frames.
    pub(crate) input: &'a mut (dyn BufRead + Send),
    /// Where the pty's output goes, as bytes frames.
    pub(crate) output: &'a mut dyn Write,
    /// Wakes a read blocked on `input` once the command is over.
    pub(crate) unblock: &'a (dyn Fn() + Sync),
    /// True once the controller began to restart.
    pub(crate) stopping: &'a (dyn Fn() -> bool + Sync),
}

/// What the relay's threads share.
struct Shared {
    /// Kills the command; a killed command ends the relay.
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    /// Set once the command exited and the pty output is drained.
    done: AtomicBool,
    /// Why the relay was ended from this side, when it was.
    ended: Mutex<Option<AttachEnd>>,
    /// Keystroke bytes taken from the client.
    bytes_in: AtomicU64,
}

impl Shared {
    /// Ends the relay for `why` unless it is already over or ending.
    fn end(&self, why: AttachEnd) {
        if self.done.load(Ordering::SeqCst) {
            return;
        }
        let mut ended = self
            .ended
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if ended.is_none() {
            *ended = Some(why);
            let _ = self
                .killer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .kill();
        }
    }
}

/// Runs `argv` under a pty of `cols` by `rows` and relays it over `link` until it ends.
///
/// The command's exit tells the cause when nothing ended the relay first: status 255, ssh's
/// status for a dropped link, or a signal means the link; any other status means the agent.
///
/// # Errors
/// The pty cannot be opened or the command cannot start.
pub(crate) fn run(argv: &[String], cols: u16, rows: u16, link: Link<'_>) -> io::Result<AttachView> {
    let pair = native_pty_system()
        .openpty(size(cols, rows))
        .map_err(io::Error::other)?;
    let mut cmd = CommandBuilder::from_argv(argv.iter().map(Into::into).collect());
    cmd.env("TERM", TERM);
    let mut child = pair.slave.spawn_command(cmd).map_err(io::Error::other)?;
    drop(pair.slave);
    let shared = Shared {
        killer: Mutex::new(child.clone_killer()),
        done: AtomicBool::new(false),
        ended: Mutex::new(None),
        bytes_in: AtomicU64::new(0),
    };
    let started = pair
        .master
        .try_clone_reader()
        .and_then(|screen| Ok((screen, pair.master.take_writer()?)));
    let (screen, keys) = match started {
        Ok(ends) => ends,
        Err(e) => {
            shared.end(AttachEnd::Link);
            let _ = child.wait();
            return Err(io::Error::other(e));
        }
    };
    let Link {
        id,
        input,
        output,
        unblock,
        stopping,
    } = link;
    let (bytes_out, status) = thread::scope(|scope| {
        let master = pair.master;
        let shared = &shared;
        scope.spawn(move || {
            take_input(id, input, keys, master.as_ref(), shared);
            shared.end(AttachEnd::Client);
        });
        scope.spawn(move || watch_stop(stopping, shared));
        let bytes_out = pump(id, screen, output, shared);
        let status = child.wait();
        shared.done.store(true, Ordering::SeqCst);
        unblock();
        (bytes_out, status)
    });
    let ended = shared
        .ended
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Ok(AttachView {
        ended_by: ended.unwrap_or_else(|| cause(status.ok().as_ref())),
        bytes_out,
        bytes_in: shared.bytes_in.into_inner(),
    })
}

/// The cause the command's exit `status` tells; an unknown status counts as the link.
fn cause(status: Option<&ExitStatus>) -> AttachEnd {
    match status {
        Some(s) if s.signal().is_none() && s.exit_code() != 255 => AttachEnd::Agent,
        _ => AttachEnd::Link,
    }
}

/// A pty size of `cols` by `rows`.
const fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Copies the pty's output to `output` as bytes frames until the pty closes; returns the
/// bytes copied. A client that stops taking output ends the relay.
fn pump(
    id: &str,
    mut screen: Box<dyn Read + Send>,
    output: &mut dyn Write,
    shared: &Shared,
) -> u64 {
    let mut buf = [0u8; 8192];
    let mut sent = 0u64;
    let mut open = true;
    loop {
        let n = match screen.read(&mut buf) {
            Ok(0) => return sent,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return sent,
        };
        let chunk = buf.get(..n).unwrap_or_default();
        if open
            && write_bytes(output, id, chunk)
                .and_then(|()| output.flush())
                .is_ok()
        {
            sent += n as u64;
        } else if open {
            open = false;
            shared.end(AttachEnd::Client);
        }
    }
}

/// Feeds the client's bytes frames to the pty and its resize frames to `master` until the
/// client shuts its side or sends anything else.
fn take_input(
    id: &str,
    input: &mut (dyn BufRead + Send),
    mut keys: Box<dyn Write + Send>,
    master: &dyn MasterPty,
    shared: &Shared,
) {
    let mut line = String::new();
    loop {
        line.clear();
        if !matches!(input.read_line(&mut line), Ok(n) if n > 0) {
            return;
        }
        match parse_frame(line.trim_end()) {
            Ok(ClientFrame::Bytes { id: got, len }) if got == id && len <= BYTES_MAX => {
                let mut buf = vec![0u8; len];
                if input.read_exact(&mut buf).is_err()
                    || keys.write_all(&buf).and_then(|()| keys.flush()).is_err()
                {
                    return;
                }
                shared.bytes_in.fetch_add(len as u64, Ordering::SeqCst);
            }
            Ok(ClientFrame::Resize {
                id: got,
                cols,
                rows,
            }) if got == id => {
                let _ = master.resize(size(cols, rows));
            }
            _ => return,
        }
    }
}

/// Ends the relay as the link's once `stopping` turns true, until the relay is over.
fn watch_stop(stopping: &(dyn Fn() -> bool + Sync), shared: &Shared) {
    while !shared.done.load(Ordering::SeqCst) {
        if stopping() {
            shared.end(AttachEnd::Link);
            return;
        }
        thread::sleep(STOP_POLL);
    }
}

#[cfg(test)]
#[path = "relay_tests.rs"]
mod tests;
