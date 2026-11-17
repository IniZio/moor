//! Commands in a Box place: one `exec` stream per command, its stdio carried as frames.

use std::fs::File;
use std::io::{self, Read, Write};
use std::time::Duration;

use moor_client::link::boxes::{BoxAnswer, BoxOp};
use moor_client::link::chan::Exit;
use moor_core::name::BoxHandle;
use moor_core::seam::driver::{DriverError, ExecOutcome, ExecSpec};
use moor_core::seam::proc::{Deadline, Output, Pipes};
use tokio::sync::mpsc;

use super::stream::{ask, blocking, failed, open, pump, Ended, Sink, Timer, Up};
use super::Machines;

/// Bytes read from a command's stdin at once.
const CHUNK: usize = 16 * 1024;

/// The caller's ends of a command's stdio.
pub(super) struct Io<'a> {
    /// Bytes for its stdin; none leaves stdin empty.
    pub(super) stdin: Option<Box<dyn Read + Send>>,
    /// Where its stdout goes.
    pub(super) out: &'a mut (dyn Write + Send),
    /// Where its stderr goes.
    pub(super) err: &'a mut (dyn Write + Send),
    /// True once the caller is gone and the command must end.
    pub(super) stop: &'a (dyn Fn() -> bool + Sync),
}

impl Machines {
    /// Runs `spec` in its place of the Box `handle` names, its stdio carried through `io`.
    pub(super) fn run(
        &self,
        handle: &BoxHandle,
        spec: &ExecSpec,
        io: Io<'_>,
    ) -> Result<ExecOutcome, DriverError> {
        let (machine, slug) = Self::seat(handle)?;
        let runner = self.runner(machine)?;
        let op = BoxOp::Exec {
            slug: slug.to_owned(),
            place: spec.place,
            argv: spec.argv.clone(),
            env: spec.env.clone(),
            cwd: spec.cwd.as_ref().map(|c| c.as_str().to_owned()),
            stdin: io.stdin.is_some(),
            tty: None,
        };
        let Io {
            stdin,
            out,
            err,
            stop,
        } = io;
        let (tx, rx) = mpsc::channel(8);
        let sending = stdin.is_some();
        if let Some(stdin) = stdin {
            feed(stdin, tx);
        }
        let ended = blocking(async {
            let mut conn = open(&runner, machine).await?;
            let mut note = |_: &str| {};
            match ask(&mut conn, &op, &mut note).await? {
                BoxAnswer::Streaming => {}
                BoxAnswer::Gone => return Err(DriverError::NotFound),
                other => return Err(failed(format!("the machine answered {other:?} to an exec"))),
            }
            pump(conn, (rx, sending), Sink { out, err }, stop).await
        })?;
        match ended {
            Ended::Exit(Exit::Code(code)) => Ok(ExecOutcome::Exited { code }),
            Ended::Exit(Exit::Signal(_)) | Ended::Stopped => Ok(ExecOutcome::Interrupted),
            Ended::Exit(Exit::Down) => Err(DriverError::Unavailable(
                "the Box's work place is down; its machine is bringing it back".to_owned(),
            )),
        }
    }

    /// [`Self::run`] with the output of `spec` sent where it says, within `deadline`.
    pub(super) fn run_captured(
        &self,
        handle: &BoxHandle,
        spec: &ExecSpec,
        deadline: Deadline,
    ) -> Result<ExecOutcome, DriverError> {
        let (mut out, mut err) = outputs(spec.output.as_ref())?;
        let Deadline(within) = deadline;
        let timer = Timer::after(within);
        let stop = move || timer.is_up();
        let io = Io {
            stdin: None,
            out: &mut *out,
            err: &mut *err,
            stop: &stop,
        };
        let outcome = self.run(handle, spec, io)?;
        if outcome == ExecOutcome::Interrupted && timer.is_up() {
            return Err(DriverError::Timeout(within));
        }
        Ok(outcome)
    }

    /// [`Self::run`] with `pipes` as its stdio.
    pub(super) fn run_piped(
        &self,
        handle: &BoxHandle,
        spec: &ExecSpec,
        pipes: Pipes<'_>,
    ) -> Result<ExecOutcome, DriverError> {
        let io = Io {
            stdin: Some(pipes.stdin),
            out: pipes.stdout,
            err: pipes.stderr,
            stop: pipes.stop,
        };
        self.run(handle, spec, io)
    }
}

/// A command's stdout or stderr writer.
type Sinks = Box<dyn Write + Send>;

/// Where a command's stdout and stderr go for `output`: nowhere when unset.
fn outputs(output: Option<&Output>) -> Result<(Sinks, Sinks), DriverError> {
    Ok(match output {
        None => (Box::new(io::sink()), Box::new(io::sink())),
        Some(Output::Inherit) => (Box::new(io::stdout()), Box::new(io::stderr())),
        Some(Output::Log(path)) => {
            let log = File::create(path)
                .map_err(|e| failed(format!("cannot write the command's log ({})", e.kind())))?;
            let more = log
                .try_clone()
                .map_err(|e| failed(format!("cannot write the command's log ({})", e.kind())))?;
            (Box::new(log), Box::new(more))
        }
    })
}

/// Reads `stdin` on a thread of its own, a blocking read that may never return must not hold the
/// caller, and sends what it reads, then the end, to the stream.
pub(super) fn feed(mut stdin: Box<dyn Read + Send>, tx: mpsc::Sender<Up>) {
    std::thread::spawn(move || {
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = stdin.read(&mut buf).unwrap_or(0);
            if n == 0 {
                let _ = tx.blocking_send(Up::End);
                return;
            }
            let bytes = buf.get(..n).unwrap_or_default().to_vec();
            if tx.blocking_send(Up::Data(bytes)).is_err() {
                return;
            }
        }
    });
}

/// How long a short call may take unless its caller says.
pub(super) const SHORT: Duration = Duration::from_secs(30);
