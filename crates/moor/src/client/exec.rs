//! `exec` through the controller: the caller's stdin goes to a command in a Box and its stdout
//! and stderr come back apart, byte for byte.

use std::io::{BufRead, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use moor_core::controller::{ClientFrame, ServerFrame, BYTES_MAX};
use moor_core::{ExecReq, Request, Response};

use super::{
    handshake, next_frame, redialing, refused, send, unexpected, with_transport, ClientError,
    SessionError, Target, REQUEST_ID,
};
use crate::env::Env;

/// How often the stdin sender checks whether the command ended while stdin stays open.
const DONE_POLL: Duration = Duration::from_millis(10);

/// The caller's ends of an `exec`.
pub struct ExecIo<'a> {
    /// What the command reads; `None` closes its stdin at once.
    pub stdin: Option<Box<dyn Read + Send>>,
    /// Where the command's stdout goes.
    pub stdout: &'a mut (dyn Write + Send),
    /// Where the command's stderr goes.
    pub stderr: &'a mut (dyn Write + Send),
}

/// Runs `req` on the controller at `target` with the command's stdio on `io`, and returns the
/// controller's answer, which carries the command's exit code. Only the dial is retried.
///
/// # Errors
/// [`ClientError`] when the controller cannot be reached, the session breaks, or `io`'s
/// stdout or stderr stops taking output.
pub fn exec(
    env: &Env,
    target: &Target,
    req: ExecReq,
    io: ExecIo<'_>,
) -> Result<Response, ClientError> {
    let request = Request::Exec(req);
    let chunks = Mutex::new(io.stdin.map(read_chunks));
    let (stdout, stderr) = (io.stdout, io.stderr);
    redialing(
        env,
        target,
        |_| {},
        || {
            with_transport(env, target, false, |reader, writer| {
                handshake(&mut &mut *reader, &mut &mut *writer)?;
                let frame = ClientFrame::Request {
                    id: REQUEST_ID.to_owned(),
                    request: Box::new(request.clone()),
                };
                send(&mut &mut *writer, &frame).map_err(ClientError::Protocol)?;
                let done = AtomicBool::new(false);
                thread::scope(|s| {
                    s.spawn(|| feed(writer, &chunks, &done));
                    let answer = answer(reader, &mut *stdout, &mut *stderr);
                    done.store(true, Ordering::SeqCst);
                    answer
                })
            })
        },
    )
}

/// Reads `stdin` on a thread of its own, which may block on it long after the command ended.
fn read_chunks(mut stdin: Box<dyn Read + Send>) -> Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::sync_channel(4);
    thread::spawn(move || {
        let mut buf = vec![0u8; BYTES_MAX];
        loop {
            let n = match stdin.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            };
            if tx.send(buf.get(..n).unwrap_or_default().to_vec()).is_err() {
                return;
            }
        }
    });
    rx
}

/// Sends stdin as bytes frames, then an eof frame once it ends, until `done`.
fn feed(
    writer: &mut (dyn Write + Send),
    chunks: &Mutex<Option<Receiver<Vec<u8>>>>,
    done: &AtomicBool,
) {
    let chunks = chunks.lock().unwrap_or_else(PoisonError::into_inner);
    let id = REQUEST_ID.to_owned();
    while let Some(chunks) = chunks.as_ref() {
        match chunks.recv_timeout(DONE_POLL) {
            Ok(data) => {
                let head = ClientFrame::Bytes {
                    id: id.clone(),
                    len: data.len(),
                };
                let sent = writer
                    .write_all(head.to_line().as_bytes())
                    .and_then(|()| writer.write_all(&data))
                    .and_then(|()| writer.flush());
                if sent.is_err() {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) if !done.load(Ordering::SeqCst) => {}
            Err(RecvTimeoutError::Timeout) => return,
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = send(&mut &mut *writer, &ClientFrame::Eof { id });
}

/// Copies the command's output frames to `stdout` and `stderr` until the answer.
fn answer(
    reader: &mut (dyn BufRead + Send),
    stdout: &mut (dyn Write + Send),
    stderr: &mut (dyn Write + Send),
) -> Result<Response, SessionError> {
    let mut output = false;
    let mut buf = Vec::new();
    loop {
        let to: &mut dyn Write = match next_frame(&mut &mut *reader)? {
            ServerFrame::Bytes { id, len } if id == REQUEST_ID && len <= BYTES_MAX => {
                buf.resize(len, 0);
                stdout
            }
            ServerFrame::Stderr { id, len } if id == REQUEST_ID && len <= BYTES_MAX => {
                buf.resize(len, 0);
                stderr
            }
            ServerFrame::Response { id, envelope } if id == REQUEST_ID => return Ok(envelope),
            other if output => return Err(unexpected(&other, "bytes, stderr or response").into()),
            other => return Err(refused(&other, "bytes, stderr or response")),
        };
        output = true;
        reader
            .read_exact(&mut buf)
            .map_err(|e| ClientError::Protocol(format!("read from controller: {e}")))?;
        to.write_all(&buf).and_then(|()| to.flush()).map_err(|e| {
            ClientError::Protocol(format!("cannot write the command's output: {e}"))
        })?;
    }
}
