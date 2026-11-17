//! `moor box-term --runner <addr> <machine-id> <slug> --place <place> -- <script>`: the argv a pane
//! runs to get a terminal in one place of a Box, through the links broker of the runner at `addr`.
//! It opens an `exec` with a terminal, puts this process's own terminal in raw mode, and carries
//! its bytes and its size changes until the command ends.

use std::io::{self, Write};
use std::os::fd::AsFd;

use moor_client::link::boxes::{BoxAnswer, BoxOp, Size};
use moor_client::link::chan::Exit;
use moor_client::link::reach::Runner;
use moor_core::seam::driver::{DriverError, Place};
use rustix::termios::{tcgetattr, tcgetwinsize, tcsetattr, OptionalActions, Termios, Winsize};
use signal_hook::consts::SIGWINCH;
use signal_hook::iterator::Signals;
use tokio::sync::mpsc;

use super::exec::feed;
use super::stream::{ask, blocking, failed, open, pump, Ended, Sink, Up};
use crate::env::{Env, MACHINE_SERVICE_KEY};

/// The size when this process has no terminal to ask.
const DEFAULT_SIZE: Size = Size { cols: 80, rows: 24 };

/// Puts the terminal back as it was once the command has ended.
struct Raw(Termios);

impl Drop for Raw {
    fn drop(&mut self) {
        let _ = tcsetattr(io::stdin().as_fd(), OptionalActions::Now, &self.0);
    }
}

/// Raw mode on this process's terminal, when it has one.
fn raw() -> Option<Raw> {
    let stdin = io::stdin();
    let was = tcgetattr(stdin.as_fd()).ok()?;
    let mut mode = was.clone();
    mode.make_raw();
    tcsetattr(stdin.as_fd(), OptionalActions::Now, &mode).ok()?;
    Some(Raw(was))
}

/// This process's terminal's size.
fn size() -> Size {
    match tcgetwinsize(io::stdin().as_fd()) {
        Ok(Winsize { ws_row, ws_col, .. }) if ws_row > 0 && ws_col > 0 => Size {
            cols: ws_col,
            rows: ws_row,
        },
        _ => DEFAULT_SIZE,
    }
}

/// Runs `script` in `place` of Box `slug` on `machine_id` on this process's terminal; the exit
/// code, which is the script's, or 1 when it could not run.
#[must_use]
pub fn box_term(
    env: &Env,
    (runner, machine_id, slug): (&str, i64, &str),
    place: Place,
    script: &str,
) -> u8 {
    let said = |why: &str| {
        let _ = writeln!(io::stderr(), "moor: box-term {machine_id} {slug}: {why}");
        1
    };
    let runner = match Runner::at(runner, env.get(MACHINE_SERVICE_KEY)) {
        Ok(runner) => runner,
        Err(e) => return said(&e),
    };
    let term = env.get("TERM").map(|t| ("TERM".to_owned(), t.to_owned()));
    let op = BoxOp::Exec {
        slug: slug.to_owned(),
        place,
        argv: vec!["sh".to_owned(), "-c".to_owned(), script.to_owned()],
        env: term.into_iter().collect(),
        cwd: None,
        stdin: true,
        tty: Some(size()),
    };
    let (tx, rx) = mpsc::channel(16);
    feed(Box::new(io::stdin()), tx.clone());
    if let Ok(mut winch) = Signals::new([SIGWINCH]) {
        std::thread::spawn(move || {
            for _ in winch.forever() {
                if tx.blocking_send(Up::Resize(size())).is_err() {
                    return;
                }
            }
        });
    }
    let raw_mode = raw();
    let ended = blocking(async {
        let mut conn = open(&runner, machine_id).await?;
        let mut note = |_: &str| {};
        match ask(&mut conn, &op, &mut note).await? {
            BoxAnswer::Streaming => {}
            BoxAnswer::Gone => return Err(DriverError::NotFound),
            other => return Err(failed(format!("the machine answered {other:?}"))),
        }
        let (mut out, mut err) = (io::stdout(), io::stderr());
        pump(
            conn,
            (rx, true),
            Sink {
                out: &mut out,
                err: &mut err,
            },
            &|| false,
        )
        .await
    });
    drop(raw_mode);
    match ended {
        Ok(Ended::Exit(Exit::Code(code))) => u8::try_from(code & 0xff).unwrap_or(1),
        Ok(Ended::Exit(Exit::Signal(n))) => u8::try_from(128 + n).unwrap_or(1),
        Ok(Ended::Exit(Exit::Down)) => said("the Box's work place is down; try again in a moment"),
        Ok(Ended::Stopped) => 1,
        Err(DriverError::NotFound) => said("the machine no longer has this Box"),
        Err(e) => said(&format!("{e:?}")),
    }
}
