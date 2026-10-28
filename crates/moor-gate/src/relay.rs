//! Relays one WebSocket to the controller's front socket. Binary messages carry the socket's
//! byte stream; an empty one is a half-close, and a Close ends both directions.

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::os::unix::net::UnixStream;
use std::sync::{Mutex, PoisonError};

use rustix::event::{poll, PollFd, PollFlags};
use tungstenite::{Error, Message, WebSocket};

use crate::rewrite::Splitter;
use crate::Person;

/// The WebSocket, shared by the two directions.
type Ws = Mutex<WebSocket<TcpStream>>;

/// Relays until either side ends the connection.
pub(crate) fn run(ws: WebSocket<TcpStream>, unix: &UnixStream, person: &Person) {
    let (Ok(probe), Ok(down)) = (ws.get_ref().try_clone(), unix.try_clone()) else {
        return;
    };
    if probe.set_nonblocking(true).is_err() {
        return;
    }
    let ws = Mutex::new(ws);
    std::thread::scope(|scope| {
        scope.spawn(|| downstream(down, &ws, &probe));
        upstream(&ws, &probe, unix, person);
        let _ = unix.shutdown(Shutdown::Both);
    });
}

/// Controller to client, until the controller's side ends.
fn downstream(mut unix: UnixStream, ws: &Ws, probe: &TcpStream) {
    let mut buf = vec![0; 64 * 1024];
    loop {
        match unix.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let chunk = buf.get(..n).unwrap_or_default().to_vec();
                if !deliver(ws, probe, Some(Message::binary(chunk))) {
                    return;
                }
            }
        }
    }
    if deliver(ws, probe, Some(Message::binary(Vec::new()))) {
        let closed = lock(ws).close(None);
        if closed.is_ok() || is_blocked(closed.as_ref().err()) {
            deliver(ws, probe, None);
        }
    }
    // Wakes the upstream poll whether or not the client answers the Close.
    let _ = probe.shutdown(Shutdown::Read);
}

/// Client to controller, until the client closes or the controller stops reading.
fn upstream(ws: &Ws, probe: &TcpStream, mut unix: &UnixStream, person: &Person) {
    let mut split = Splitter::default();
    loop {
        if !wait(probe, PollFlags::IN) {
            return;
        }
        let Batch {
            up,
            back,
            half_close,
            done,
        } = drain(&mut lock(ws), &mut split, person);
        if unix.write_all(&up).is_err() {
            return;
        }
        if half_close {
            let _ = unix.shutdown(Shutdown::Write);
        }
        for reply in back {
            if !deliver(ws, probe, Some(Message::binary(reply.into_bytes()))) {
                return;
            }
        }
        if done {
            return;
        }
    }
}

/// What one burst of client messages asks for.
#[derive(Default)]
struct Batch {
    /// Bytes for the controller.
    up: Vec<u8>,
    /// Frames the gate answers itself.
    back: Vec<String>,
    /// The client shut down its write side.
    half_close: bool,
    /// The connection is over.
    done: bool,
}

/// Reads every message the client has sent so far.
fn drain(ws: &mut WebSocket<TcpStream>, split: &mut Splitter, person: &Person) -> Batch {
    let mut batch = Batch::default();
    while !batch.done {
        match ws.read() {
            Ok(Message::Binary(bytes)) if bytes.is_empty() => batch.half_close = true,
            Ok(Message::Binary(bytes)) => {
                if let Err(why) = split.feed(&bytes, person, &mut batch.up, &mut batch.back) {
                    crate::log(&why);
                    batch.done = true;
                }
            }
            Ok(Message::Text(_) | Message::Close(_)) => batch.done = true,
            Ok(_) => {}
            Err(e) if is_blocked(Some(&e)) => break,
            Err(_) => batch.done = true,
        }
    }
    batch
}

/// Sends `msg` (or only flushes) on the non-blocking socket, waiting while its buffer is full.
fn deliver(ws: &Ws, probe: &TcpStream, msg: Option<Message>) -> bool {
    let mut sent = msg.map_or_else(|| lock(ws).flush(), |msg| lock(ws).send(msg));
    while is_blocked(sent.as_ref().err()) {
        if !wait(probe, PollFlags::OUT) {
            return false;
        }
        sent = lock(ws).flush();
    }
    sent.is_ok()
}

/// Whether `e` only says the non-blocking socket would block.
fn is_blocked(e: Option<&Error>) -> bool {
    matches!(e, Some(Error::Io(io)) if io.kind() == ErrorKind::WouldBlock)
}

/// Waits until `probe` is ready for `flags`; false when the wait failed.
fn wait(probe: &TcpStream, flags: PollFlags) -> bool {
    poll(&mut [PollFd::new(probe, flags)], None).is_ok()
}

/// The WebSocket, even when another thread panicked holding it.
fn lock(ws: &Ws) -> std::sync::MutexGuard<'_, WebSocket<TcpStream>> {
    ws.lock().unwrap_or_else(PoisonError::into_inner)
}
