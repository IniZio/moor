//! Tests for the three-step file upload against loopback HTTP fakes.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use moor_core::secret::Secret;

use super::{request_complete, TOKEN};
use crate::api::{Api, ApiError};

/// One request a [`serve_routes`] fake received.
struct Seen {
    /// Request path.
    path: String,
    /// Lower-cased header block, including the request line.
    head: String,
    /// Request body.
    body: Vec<u8>,
}

/// Writes a JSON-typed HTTP reply with `status` and `reply` and closes.
fn respond(conn: &mut TcpStream, status: &str, reply: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
        reply.len()
    );
    conn.write_all(response.as_bytes()).expect("write");
}

/// Serves `count` requests on loopback, answering each by path from `routes`.
///
/// A route is `(path, status, reply)`; `{BASE}` in a reply becomes the fake's base URL.
fn serve_routes(
    count: usize,
    routes: &'static [(&'static str, &'static str, &'static str)],
) -> (String, JoinHandle<Vec<Seen>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let own = base.clone();
    let handle = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for _ in 0..count {
            let (mut conn, _) = listener.accept().expect("accept");
            conn.set_read_timeout(Some(Duration::from_secs(2)))
                .expect("timeout");
            let mut request = Vec::new();
            let mut buf = [0_u8; 4096];
            while !request_complete(&request) {
                let n = conn.read(&mut buf).expect("read");
                assert!(n > 0, "client closed early");
                request.extend_from_slice(buf.get(..n).expect("slice"));
            }
            let split = request
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .expect("head end");
            let head = String::from_utf8_lossy(request.get(..split).expect("head")).to_lowercase();
            let path = head.split_whitespace().nth(1).expect("path").to_owned();
            let (status, reply) = routes
                .iter()
                .find(|(p, _, _)| *p == path)
                .map(|(_, s, r)| (*s, r.replace("{BASE}", &own)))
                .expect("route");
            respond(&mut conn, status, &reply);
            seen.push(Seen {
                path,
                head,
                body: request.get(split + 4..).expect("body").to_vec(),
            });
        }
        seen
    });
    (base, handle)
}

const UPLOAD_ROUTES: &[(&str, &str, &str)] = &[
    (
        "/files.getuploadurlexternal",
        "200 OK",
        r#"{"ok":true,"upload_url":"{BASE}/put/F1","file_id":"F1"}"#,
    ),
    ("/put/f1", "200 OK", "OK - 5"),
    ("/files.completeuploadexternal", "200 OK", r#"{"ok":true}"#),
];

/// Runs the three upload calls for `len` bytes of `body` as `name` into thread `1.1` of `C1`.
fn upload_from(base: &str, name: &str, len: u64, body: &mut dyn Read) -> Result<(), ApiError> {
    let (api, bot) = (
        Api::new(base).expect("api"),
        Secret::new(TOKEN.as_bytes().to_vec()),
    );
    let to = api.upload_url(&bot, name, len)?;
    api.send_file(&to, len, body)?;
    api.complete_upload(&bot, "C1", "1.1", &to, name)
}

/// Uploads `bytes` as `name` into thread `1.1` of `C1` through `base`.
fn upload(base: &str, name: &str, mut bytes: &[u8]) -> Result<(), ApiError> {
    upload_from(base, name, bytes.len() as u64, &mut bytes)
}

#[test]
fn upload_file_runs_the_three_calls_in_order() {
    let (base, handle) = serve_routes(3, UPLOAD_ROUTES);
    upload(&base, "shot one.png", b"hello").expect("upload");
    let seen = handle.join().expect("server");
    let paths: Vec<&str> = seen.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "/files.getuploadurlexternal",
            "/put/f1",
            "/files.completeuploadexternal"
        ]
    );
    let [get, put, done] = seen.as_slice() else {
        unreachable!("three requests")
    };
    assert!(get.head.contains(&format!("authorization: bearer {TOKEN}")));
    assert_eq!(
        String::from_utf8_lossy(&get.body),
        "filename=shot%20one.png&length=5"
    );
    assert_eq!(put.body, b"hello");
    assert!(put.head.contains("content-length: 5"), "{}", put.head);
    assert!(done
        .head
        .contains(&format!("authorization: bearer {TOKEN}")));
    let sent: serde_json::Value = serde_json::from_slice(&done.body).expect("json");
    assert_eq!(
        sent,
        serde_json::json!({
            "files": [{"id": "F1", "title": "shot one.png"}],
            "channel_id": "C1",
            "thread_ts": "1.1",
        })
    );
}

#[test]
fn upload_post_carries_no_authorization_header() {
    let (base, handle) = serve_routes(3, UPLOAD_ROUTES);
    upload(&base, "a.txt", b"hello").expect("upload");
    let seen = handle.join().expect("server");
    let put = seen.get(1).expect("put");
    assert!(!put.head.contains("authorization"), "{}", put.head);
    assert!(!put.head.contains(&TOKEN.to_lowercase()), "{}", put.head);
}

#[test]
fn upload_url_refusal_is_a_slack_error() {
    static ROUTES: &[(&str, &str, &str)] = &[(
        "/files.getuploadurlexternal",
        "200 OK",
        r#"{"ok":false,"error":"missing_scope"}"#,
    )];
    let (base, handle) = serve_routes(1, ROUTES);
    let err = upload(&base, "a.txt", b"x").expect_err("rejected");
    handle.join().expect("server");
    assert!(
        matches!(&err, ApiError::Slack(code) if code == "missing_scope"),
        "{err:?}"
    );
}

#[test]
fn refused_upload_is_an_error_without_the_token_and_skips_completion() {
    static ROUTES: &[(&str, &str, &str)] = &[
        (
            "/files.getuploadurlexternal",
            "200 OK",
            r#"{"ok":true,"upload_url":"{BASE}/put/F1","file_id":"F1"}"#,
        ),
        (
            "/put/f1",
            "500 Internal Server Error",
            "denied for Authorization: Bearer xoxb-super-secret-token",
        ),
    ];
    let (base, handle) = serve_routes(2, ROUTES);
    let err = upload(&base, "a.txt", b"x").expect_err("refused");
    handle.join().expect("server");
    assert!(
        matches!(&err, ApiError::Transport(m) if m.contains("500")),
        "{err:?}"
    );
    assert!(!format!("{err} {err:?}").contains(TOKEN), "{err}");
}

/// Bytes the client may hold, read from the body but not yet received by the fake.
const WINDOW: u64 = 256 * 1024;
/// Body length: far more than the window and than any buffer the client keeps.
const TOTAL: u64 = 16 * 1024 * 1024;

/// Body bytes read by the client and received by the fake, shared by both sides.
#[derive(Default)]
struct Flow {
    /// Bytes the client read from the body.
    produced: u64,
    /// Bytes the fake received.
    received: u64,
}

/// A shared [`Flow`] and the signal that it changed.
type Shared = Arc<(Mutex<Flow>, Condvar)>;

/// A body of `TOTAL` bytes that fails a read more than [`WINDOW`] ahead of the fake.
struct Paced(Shared);

impl Read for Paced {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let (lock, changed) = &*self.0;
        let ahead = |f: &mut Flow| f.produced < TOTAL && f.produced - f.received >= WINDOW;
        let (mut flow, wait) = changed
            .wait_timeout_while(lock.lock().expect("lock"), Duration::from_secs(5), ahead)
            .expect("wait");
        if wait.timed_out() {
            return Err(io::Error::other(
                "the client read the body ahead of sending it",
            ));
        }
        let room = (WINDOW - (flow.produced - flow.received)).min(TOTAL - flow.produced);
        let n = buf.len().min(usize::try_from(room).expect("room"));
        flow.produced += n as u64;
        drop(flow);
        buf.get_mut(..n).expect("room").fill(b'x');
        Ok(n)
    }
}

/// Reads one request head; the lower-cased head text.
fn read_head(conn: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        conn.read_exact(&mut byte).expect("head");
        head.extend_from_slice(&byte);
    }
    String::from_utf8_lossy(&head).to_lowercase()
}

/// The Content-Length of a lower-cased head.
fn content_length(head: &str) -> u64 {
    head.lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .map_or(0, |v| v.trim().parse().expect("length"))
}

/// Serves the three upload calls, counting each body chunk of the file POST into `flow`
/// as it arrives; the handle yields the file POST's Content-Length and every byte was `x`.
fn serve_paced(flow: Shared) -> (String, JoinHandle<(u64, bool)>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let own = base.clone();
    let handle = std::thread::spawn(move || {
        let mut result = (0, false);
        for _ in 0..3 {
            let (mut conn, _) = listener.accept().expect("accept");
            let head = read_head(&mut conn);
            let mut left = content_length(&head);
            let file = head.starts_with("post /put/");
            let (mut chunk, mut all_x) = (vec![0_u8; 64 * 1024], true);
            while left > 0 {
                let want = chunk.len().min(usize::try_from(left).expect("left"));
                let got = chunk.get_mut(..want).expect("want");
                let n = conn.read(got).expect("body");
                assert!(n > 0, "client closed early");
                left -= n as u64;
                if file {
                    all_x &= got.get(..n).expect("n").iter().all(|b| *b == b'x');
                    let (lock, changed) = &*flow;
                    lock.lock().expect("lock").received += n as u64;
                    changed.notify_all();
                }
            }
            let reply = if file {
                result = (content_length(&head), all_x);
                "OK".to_owned()
            } else if head.starts_with("post /files.getuploadurlexternal") {
                format!(r#"{{"ok":true,"upload_url":"{own}/put/F1","file_id":"F1"}}"#)
            } else {
                r#"{"ok":true}"#.to_owned()
            };
            respond(&mut conn, "200 OK", &reply);
        }
        result
    });
    (base, handle)
}

#[test]
fn upload_streams_the_body_without_reading_ahead() {
    let flow = Shared::default();
    let (base, handle) = serve_paced(Arc::clone(&flow));
    let mut body = Paced(Arc::clone(&flow));
    upload_from(&base, "big.bin", TOTAL, &mut body).expect("upload");
    let (length, all_x) = handle.join().expect("server");
    assert_eq!(length, TOTAL, "Content-Length of the file POST");
    assert!(all_x, "the file POST carried the raw bytes");
    assert_eq!(flow.0.lock().expect("lock").received, TOTAL);
}
