#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "tests assert on fixed fixtures"
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use moor_core::controller::{parse_frame, ClientFrame, PROTOCOL};

use super::*;
use crate::http::start;

const TOKEN: &str = "t0ken";
const SHORT: Duration = Duration::from_millis(30);
const LONG: Duration = Duration::from_secs(60);

/// What the fake controller serves and what it saw.
#[derive(Clone, Default)]
struct Fake {
    /// Lines to send, each with the cursor it is stored under.
    lines: Arc<Mutex<Vec<(i64, String)>>>,
    /// Watch frames received, one per connection.
    watches: Arc<Mutex<Vec<ClientFrame>>>,
    /// Cursor after which the next connection is dropped, taken once.
    cut: Arc<Mutex<Option<i64>>>,
    /// Connections the browser side has closed, seen as EOF.
    closed: Arc<AtomicUsize>,
}

impl Fake {
    fn push_event(&self, cursor: i64) {
        let line = format!(
            r#"{{"frame":"event","cursor":{cursor},"kind":"box_ready","payload":{{"n":{cursor}}},"ts":{cursor}}}"#
        );
        self.lines.lock().unwrap().push((cursor, line));
    }

    fn push_error(&self) {
        let line = r#"{"frame":"error","kind":"slow-client","message":"too slow"}"#;
        self.lines.lock().unwrap().push((i64::MAX, line.to_owned()));
    }

    fn last_watch(&self) -> ClientFrame {
        self.watches.lock().unwrap().last().cloned().unwrap()
    }
}

/// Answers hello, records the watch line, then sends stored lines after `since` as they
/// appear, holding the connection open until a write fails.
fn serve(fake: &Fake, socket: &Path) {
    let listener = UnixListener::bind(socket).unwrap();
    let fake = fake.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let fake = fake.clone();
            std::thread::spawn(move || converse(&fake, stream.unwrap()));
        }
    });
}

fn converse(fake: &Fake, stream: UnixStream) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    writeln!(writer, r#"{{"frame":"hello","protocol":{PROTOCOL}}}"#).unwrap();
    line.clear();
    reader.read_line(&mut line).unwrap();
    let frame = parse_frame(line.trim_end()).unwrap();
    let ClientFrame::Watch { since, .. } = frame else {
        return;
    };
    fake.watches.lock().unwrap().push(frame);
    let cut = fake.cut.lock().unwrap().take();
    let gone = Arc::new(AtomicBool::new(false));
    let (flag, closed) = (Arc::clone(&gone), Arc::clone(&fake.closed));
    std::thread::spawn(move || {
        let _ = reader.read_line(&mut String::new());
        flag.store(true, Ordering::SeqCst);
        closed.fetch_add(1, Ordering::SeqCst);
    });
    let mut sent = 0;
    while !gone.load(Ordering::SeqCst) {
        let stored = fake.lines.lock().unwrap().clone();
        for (cursor, line) in stored.iter().skip(sent) {
            if *cursor > since && writeln!(writer, "{line}").is_err() {
                return;
            }
            if cut.is_some_and(|c| *cursor >= c) {
                let _ = writer.shutdown(std::net::Shutdown::Both);
                return;
            }
        }
        sent = stored.len();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn fake_client(dir: &tempfile::TempDir) -> (Fake, Client) {
    let sock = dir.path().join("c.sock");
    let fake = Fake::default();
    serve(&fake, &sock);
    (fake, Client::new(sock))
}

/// Opens `path` with the token and extra header lines; returns the stream after the head.
fn open(web: &crate::http::Server, path: &str, extra: &[&str]) -> BufReader<TcpStream> {
    let mut s = TcpStream::connect(web.addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut req = format!("GET {path} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\n");
    for h in extra {
        req.push_str(h);
        req.push_str("\r\n");
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).unwrap();
    let mut r = BufReader::new(s);
    let mut status = String::new();
    r.read_line(&mut status).unwrap();
    assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    loop {
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        if line == "\r\n" {
            return r;
        }
    }
}

/// Reads lines until one starts with `prefix`.
fn read_until(r: &mut impl BufRead, prefix: &str) -> String {
    loop {
        let mut line = String::new();
        assert_ne!(r.read_line(&mut line).unwrap(), 0, "stream ended");
        if line.starts_with(prefix) {
            return line;
        }
    }
}

fn config(keepalive: Duration) -> RelayConfig {
    RelayConfig {
        keepalive,
        backoff_min: Duration::from_millis(5),
        backoff_max: Duration::from_millis(40),
        channel_bound: 4,
    }
}

fn db_path(dir: &tempfile::TempDir) -> std::path::PathBuf {
    dir.path().join("web.db")
}

fn test_db(dir: &tempfile::TempDir) -> Arc<Mutex<Db>> {
    Arc::new(Mutex::new(Db::open(&db_path(dir)).unwrap()))
}

fn web(dir: &tempfile::TempDir, client: Client, keepalive: Duration) -> crate::http::Server {
    let handler = handler(client, test_db(dir), config(keepalive));
    start(0, TOKEN.to_owned(), handler).unwrap()
}

/// Polls `done` for up to five seconds.
fn eventually(mut done: impl FnMut() -> bool) -> bool {
    for _ in 0..500 {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn stream_delivers_new_events_on_the_open_connection() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, client) = fake_client(&dir);
    fake.push_event(1);
    let web = web(&dir, client, LONG);
    let mut r = open(&web, "/events", &[]);
    assert_eq!(read_until(&mut r, "id:"), "id: 1\n");
    fake.push_event(2);
    assert_eq!(read_until(&mut r, "id:"), "id: 2\n");
    assert!(read_until(&mut r, "data:").contains("\"cursor\":2"));
}

#[test]
fn last_event_id_resumes_after_that_event() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, client) = fake_client(&dir);
    for c in 1..=3 {
        fake.push_event(c);
    }
    let web = web(&dir, client, LONG);
    let mut r = open(&web, "/events", &["Last-Event-ID: 2"]);
    assert_eq!(read_until(&mut r, "id:"), "id: 3\n");
    assert!(matches!(
        fake.last_watch(),
        ClientFrame::Watch {
            since: 2,
            follow: true,
            ..
        }
    ));
    let mut r = open(&web, "/events?since=1", &[]);
    assert_eq!(read_until(&mut r, "id:"), "id: 2\n");
    assert!(matches!(
        fake.last_watch(),
        ClientFrame::Watch { since: 1, .. }
    ));
}

#[test]
fn other_paths_are_not_streamed() {
    let dir = tempfile::tempdir().unwrap();
    let (_, client) = fake_client(&dir);
    let web = web(&dir, client, LONG);
    let mut s = TcpStream::connect(web.addr()).unwrap();
    write!(
        s,
        "GET /nope HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    assert!(!out.contains("text/event-stream"), "{out}");
}

#[test]
fn relay_writes_an_error_frame_when_the_controller_errors() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, client) = fake_client(&dir);
    fake.push_error();
    let got = Arc::default();
    let mut out = Limited {
        got: Arc::clone(&got),
        left: 3,
    };
    assert!(relay(&client, 0, &mut out, &test_db(&dir), config(SHORT)).is_err());
    let text = String::from_utf8(got.lock().unwrap().clone()).unwrap();
    assert!(text.contains("event: error"), "{text}");
    assert!(text.contains("too slow"), "{text}");
}

#[test]
fn relay_writes_an_error_frame_when_the_controller_is_unreachable() {
    let dir = tempfile::tempdir().unwrap();
    let client = Client::new(dir.path().join("missing.sock"));
    let got = Arc::default();
    let mut out = Limited {
        got: Arc::clone(&got),
        left: 3,
    };
    assert!(relay(&client, 0, &mut out, &test_db(&dir), config(SHORT)).is_err());
    let text = String::from_utf8(got.lock().unwrap().clone()).unwrap();
    assert!(text.contains("event: error"), "{text}");
}

/// Accepts writes until `left` runs out, then fails.
struct Limited {
    /// Bytes written so far.
    got: Arc<Mutex<Vec<u8>>>,
    /// Writes still allowed.
    left: usize,
}

impl Write for Limited {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.left == 0 {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        self.left -= 1;
        self.got.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn relay_writes_keepalive_while_the_controller_is_quiet() {
    let dir = tempfile::tempdir().unwrap();
    let (_, client) = fake_client(&dir);
    let got = Arc::default();
    let mut out = Limited {
        got: Arc::clone(&got),
        left: 2,
    };
    assert!(relay(&client, 0, &mut out, &test_db(&dir), config(SHORT)).is_err());
    let text = String::from_utf8(got.lock().unwrap().clone()).unwrap();
    assert_eq!(text.as_bytes(), [sse::KEEPALIVE, sse::KEEPALIVE].concat());
}

#[test]
fn relay_returns_when_the_writer_fails() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, client) = fake_client(&dir);
    fake.push_event(1);
    let mut out = Limited {
        got: Arc::default(),
        left: 0,
    };
    let err = relay(&client, 0, &mut out, &test_db(&dir), config(LONG)).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
}

#[test]
fn delivered_cursor_is_saved() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, client) = fake_client(&dir);
    fake.push_event(1);
    fake.push_event(2);
    let web = web(&dir, client, LONG);
    let mut r = open(&web, "/events", &[]);
    assert_eq!(read_until(&mut r, "id:"), "id: 1\n");
    assert_eq!(read_until(&mut r, "id:"), "id: 2\n");
    let db = Db::open(&db_path(&dir)).unwrap();
    assert!(eventually(|| db.cursor().unwrap() == 2));
}

#[test]
fn stream_resumes_after_the_controller_drops() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, client) = fake_client(&dir);
    fake.push_event(1);
    fake.push_event(2);
    *fake.cut.lock().unwrap() = Some(2);
    let web = web(&dir, client, LONG);
    let mut r = open(&web, "/events", &[]);
    assert_eq!(read_until(&mut r, "id:"), "id: 1\n");
    assert_eq!(read_until(&mut r, "id:"), "id: 2\n");
    fake.push_event(3);
    assert_eq!(read_until(&mut r, "id:"), "id: 3\n");
    let watches = fake.watches.lock().unwrap().clone();
    assert_eq!(watches.len(), 2);
    assert!(matches!(watches[1], ClientFrame::Watch { since: 2, .. }));
}

#[test]
fn closing_the_browser_closes_a_quiet_controller_connection() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, client) = fake_client(&dir);
    let web = web(&dir, client, SHORT);
    let r = open(&web, "/events", &[]);
    assert!(eventually(|| !fake.watches.lock().unwrap().is_empty()));
    drop(r);
    assert!(eventually(|| fake.closed.load(Ordering::SeqCst) == 1));
}

#[test]
fn closing_the_browser_closes_an_active_controller_connection() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, client) = fake_client(&dir);
    fake.push_event(1);
    let web = web(&dir, client, LONG);
    let mut r = open(&web, "/events", &[]);
    assert_eq!(read_until(&mut r, "id:"), "id: 1\n");
    drop(r);
    let mut next = 2;
    assert!(eventually(|| {
        fake.push_event(next);
        next += 1;
        fake.closed.load(Ordering::SeqCst) >= 1
    }));
}
