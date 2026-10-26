//! The fake controller socket: streams stored artifact bytes, one connection per answer.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index built values"
)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::thread::JoinHandle;
use std::time::Duration;

use moor_core::controller::{ServerFrame, BYTES_MAX, PROTOCOL};
use moor_core::{ErrorKind, ErrorView, Response};
use serde_json::{json, Value};

/// What the fake controller streams for one connection: `pattern` repeated up to `total`
/// bytes, in bytes frames of `frame` bytes, then an ok response or the error `fail`.
pub(crate) struct Stored {
    /// Bytes repeated to make the file.
    pub(super) pattern: Vec<u8>,
    /// File length.
    pub(super) total: u64,
    /// Bytes per frame.
    pub(super) frame: usize,
    /// Error message of the final response, if it fails.
    pub(super) fail: Option<&'static str>,
}

impl Stored {
    /// A stored copy that cannot be read: the controller answers with `message`.
    pub(crate) fn failing(message: &'static str) -> Self {
        Self {
            pattern: vec![0],
            total: 0,
            frame: BYTES_MAX,
            fail: Some(message),
        }
    }

    /// `bytes` as one file in full frames.
    pub(crate) fn of(bytes: &[u8]) -> Self {
        Self {
            pattern: bytes.to_vec(),
            total: bytes.len() as u64,
            frame: BYTES_MAX,
            fail: None,
        }
    }
}

/// Writes the frames of `s` to `out`.
fn send(out: &mut impl Write, s: &Stored) -> std::io::Result<()> {
    let (mut at, mut sent, mut frame) = (0, 0_u64, Vec::with_capacity(s.frame));
    while sent < s.total {
        let want = s.frame.min(usize::try_from(s.total - sent).unwrap());
        frame.clear();
        while frame.len() < want {
            let take = (want - frame.len()).min(s.pattern.len() - at);
            frame.extend_from_slice(&s.pattern[at..at + take]);
            at = (at + take) % s.pattern.len();
        }
        let head = ServerFrame::Bytes {
            id: "1".into(),
            len: want,
        };
        out.write_all(head.to_line().as_bytes())?;
        out.write_all(&frame)?;
        sent += want as u64;
    }
    let envelope = s.fail.map_or_else(
        || Response::ok("artifact-read", Some(json!({"sent": sent, "eof": true}))),
        |message| {
            let error = ErrorView {
                kind: ErrorKind::Failed,
                message: message.to_owned(),
                hints: Vec::new(),
                context: Vec::new(),
                detail: None,
            };
            Response::err("artifact-read", error)
        },
    );
    let end = ServerFrame::Response {
        id: "1".into(),
        envelope,
    };
    out.write_all(end.to_line().as_bytes())
}

/// Answers the hello on `conn` and returns the request that follows it.
fn take_request(mut conn: &UnixStream) -> Value {
    let mut reader = BufReader::new(conn);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let hello = ServerFrame::Hello { protocol: PROTOCOL };
    conn.write_all(hello.to_line().as_bytes()).unwrap();
    line.clear();
    reader.read_line(&mut line).unwrap();
    serde_json::from_str::<Value>(&line).unwrap()["request"].clone()
}

/// Serves one connection per entry of `answers` on `sock`, then records any request that
/// arrives within 300 ms and answers none. Yields each request received.
pub(crate) fn controller(sock: &Path, answers: Vec<Stored>) -> JoinHandle<Vec<Value>> {
    let listener = UnixListener::bind(sock).unwrap();
    std::thread::spawn(move || {
        let mut seen = Vec::new();
        for stored in answers {
            let (mut conn, _) = listener.accept().unwrap();
            seen.push(take_request(&conn));
            let _ = send(&mut conn, &stored);
        }
        listener.set_nonblocking(true).unwrap();
        for _ in 0..6 {
            std::thread::sleep(Duration::from_millis(50));
            if let Ok((conn, _)) = listener.accept() {
                conn.set_nonblocking(false).unwrap();
                seen.push(take_request(&conn));
            }
        }
        seen
    })
}
