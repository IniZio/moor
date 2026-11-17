//! One Box request on a stream the runner splices to a machine's agent: the request line, the
//! answer lines, and for an `exec` the frames both ways.

use std::future::Future;
use std::io::Write;
use std::time::Duration;

use moor_client::link::boxes::{BoxAnswer, BoxOp, Size};
use moor_client::link::broker;
use moor_client::link::chan::{read_frame, write_frame, Exit, Frame};
use moor_client::link::ops;
use moor_client::link::reach::{Conn, Runner};
use moor_core::seam::driver::DriverError;
use tokio::sync::mpsc;

/// How often a running command asks whether its caller still wants it.
const STOP_POLL: Duration = Duration::from_millis(100);

/// Runs `call` to completion on a runtime of its own, off any runtime the caller is on.
pub(super) fn blocking<T: Send>(
    call: impl Future<Output = Result<T, DriverError>> + Send,
) -> Result<T, DriverError> {
    std::thread::scope(|s| {
        s.spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| failed(format!("cannot start a runtime: {e}")))?;
            let done = runtime.block_on(call);
            runtime.shutdown_background();
            done
        })
        .join()
        .unwrap_or_else(|_| Err(failed("the Box call panicked".to_owned())))
    })
}

/// A wait that ends after a length of real time, which a stream's deadline is, never a reading
/// of the Clock seam.
#[derive(Clone, Copy)]
pub(super) struct Timer(std::time::Instant);

impl Timer {
    /// A timer that is up after `within`.
    #[expect(
        clippy::disallowed_methods,
        reason = "a stream's deadline waits real time"
    )]
    pub(super) fn after(within: Duration) -> Self {
        Self(std::time::Instant::now() + within)
    }

    /// Whether the time is up.
    #[expect(
        clippy::disallowed_methods,
        reason = "a stream's deadline waits real time"
    )]
    pub(super) fn is_up(self) -> bool {
        std::time::Instant::now() >= self.0
    }
}

/// A [`DriverError::Failed`] saying `summary`.
pub(super) const fn failed(summary: String) -> DriverError {
    DriverError::Failed { summary }
}

/// What a broker or stream failure `reason` means for the call: a machine or runner out of reach
/// is unavailable, anything else the machine said is a failure.
pub(super) fn unreached(reason: String) -> DriverError {
    if broker::is_link_loss(&reason) {
        DriverError::Unavailable(reason)
    } else {
        failed(reason)
    }
}

/// A stream to `machine`'s agent that carries one request.
pub(super) async fn open(runner: &Runner, machine: i64) -> Result<Conn, DriverError> {
    broker::boxes(runner, machine).await.map_err(unreached)
}

/// Sends `op` and reads answers until one that is not a note, handing each note to `note`.
/// A refusal is an error.
pub(super) async fn ask(
    conn: &mut Conn,
    op: &BoxOp,
    note: &mut (dyn FnMut(&str) + Send),
) -> Result<BoxAnswer, DriverError> {
    ops::write_line(conn, op)
        .await
        .map_err(|e| DriverError::Transport {
            stderr_tail: format!("the machine's stream closed: {e}"),
        })?;
    loop {
        let answer: BoxAnswer = ops::read_line(conn).await.map_err(unreached)?;
        match answer {
            BoxAnswer::Note { line } => note(&line),
            BoxAnswer::Refused { reason } => return Err(failed(reason)),
            other => return Ok(other),
        }
    }
}

/// What the controller's side sends a running command.
pub(super) enum Up {
    /// Bytes for stdin.
    Data(Vec<u8>),
    /// Stdin ends.
    End,
    /// The terminal's new size.
    Resize(Size),
}

/// Where a running command's output goes.
pub(super) struct Sink<'a> {
    /// Its stdout, or its terminal.
    pub(super) out: &'a mut (dyn Write + Send),
    /// Its stderr.
    pub(super) err: &'a mut (dyn Write + Send),
}

/// How a carried command ended.
pub(super) enum Ended {
    /// It exited, or was ended by a signal, or its place was down.
    Exit(Exit),
    /// The caller said stop, or took no more output; the stream drops, which ends the command.
    Stopped,
}

/// What the stream's reader hands the pump.
enum Heard {
    /// Stdout bytes.
    Out(Vec<u8>),
    /// Stderr bytes.
    Err(Vec<u8>),
    /// The command ended.
    Exit(Exit),
    /// The stream ended or broke before an exit.
    Cut(String),
}

/// Carries a command's stdio over `conn`, which already answered `streaming`, until it exits or
/// `stop` says to end it. `stdin` is whether the request promised `in` frames; `up` feeds them,
/// and a terminal's resizes. Stdin ends with an `in-end` frame and the stream is never
/// half-closed, because the agent reads a closed stream as a controller that is gone and ends the
/// command's process group.
pub(super) async fn pump(
    conn: Conn,
    (mut up, mut sending): (mpsc::Receiver<Up>, bool),
    sink: Sink<'_>,
    stop: &(dyn Fn() -> bool + Sync),
) -> Result<Ended, DriverError> {
    let (mut rd, mut wr) = tokio::io::split(conn);
    let (tx, mut heard) = mpsc::channel(8);
    let reader = tokio::spawn(async move {
        let mut buf = Vec::new();
        loop {
            let said = match read_frame(&mut rd, &mut buf).await {
                Ok(Some(Frame::Out(b))) => Heard::Out(b.to_vec()),
                Ok(Some(Frame::Err(b))) => Heard::Err(b.to_vec()),
                Ok(Some(Frame::Exit(e))) => Heard::Exit(e),
                Ok(Some(_)) => continue,
                Ok(None) => Heard::Cut("the stream ended before the command did".to_owned()),
                Err(e) => Heard::Cut(e.to_string()),
            };
            let last = matches!(said, Heard::Exit(_) | Heard::Cut(_));
            if tx.send(said).await.is_err() || last {
                return;
            }
        }
    });
    let mut sink = sink;
    let mut poll = tokio::time::interval(STOP_POLL);
    let ended = loop {
        tokio::select! {
            said = heard.recv() => if let Some(ended) = deliver(said, &mut sink) {
                break ended;
            },
            fed = up.recv(), if sending => {
                if let Err(e) = send(&mut wr, fed, &mut sending).await {
                    break Err(e);
                }
            }
            _ = poll.tick() => if stop() {
                break Ok(Ended::Stopped);
            },
        }
    };
    reader.abort();
    ended
}

/// Sends the controller's side `fed` to the command; clears `sending` once stdin has ended.
async fn send<W: tokio::io::AsyncWrite + Unpin>(
    wr: &mut W,
    fed: Option<Up>,
    sending: &mut bool,
) -> Result<(), DriverError> {
    let frame = match &fed {
        Some(Up::Data(b)) => Frame::In(b),
        Some(Up::Resize(size)) => Frame::Resize(*size),
        Some(Up::End) | None => {
            *sending = false;
            Frame::InEnd
        }
    };
    write_frame(wr, &frame)
        .await
        .map_err(|e| cut(format!("the machine's stream closed: {e}")))
}

/// The error for a stream cut for `why`, whether the command ran to its end being unknown.
const fn cut(why: String) -> DriverError {
    DriverError::Transport { stderr_tail: why }
}

/// Hands what the command said to `sink`; the end of the pump when it said it ended, or when the
/// sink takes no more.
fn deliver(said: Option<Heard>, sink: &mut Sink<'_>) -> Option<Result<Ended, DriverError>> {
    let write = |to: &mut (dyn Write + Send), bytes: &[u8]| {
        to.write_all(bytes)
            .and_then(|()| to.flush())
            .is_err()
            .then_some(Ok(Ended::Stopped))
    };
    match said {
        Some(Heard::Out(b)) => write(&mut *sink.out, &b),
        Some(Heard::Err(b)) => write(&mut *sink.err, &b),
        Some(Heard::Exit(e)) => Some(Ok(Ended::Exit(e))),
        Some(Heard::Cut(why)) => Some(Err(cut(why))),
        None => Some(Err(cut("the stream's reader stopped".to_owned()))),
    }
}
