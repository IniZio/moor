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
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::controller::{parse_frame, ClientFrame, PROTOCOL};
use moor_core::request::{CloseReq, EndSessionReq, Request, SteerReq};
use moor_core::response::{ErrorKind, ErrorView, Response};
use moor_core::BoxName;
use serde_json::{json, Value};

use super::handler;
use crate::client::Client;
use crate::http::{start, HttpResponse, Server};

const TOKEN: &str = "t0ken";

/// Requests the fake controller received, in arrival order.
type Seen = Arc<Mutex<Vec<Request>>>;

/// Serves hello and answers every request with `envelope`, recording the requests.
fn serve(socket: &Path, envelope: &Response) -> Seen {
    let listener = UnixListener::bind(socket).unwrap();
    let seen = Seen::default();
    let (record, envelope) = (seen.clone(), envelope.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let (record, envelope) = (record.clone(), envelope.clone());
            std::thread::spawn(move || converse(&record, &envelope, stream.unwrap()));
        }
    });
    seen
}

fn converse(seen: &Seen, envelope: &Response, stream: UnixStream) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    writeln!(writer, r#"{{"frame":"hello","protocol":{PROTOCOL}}}"#).unwrap();
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap() == 0 {
            return;
        }
        let ClientFrame::Request { id, request } = parse_frame(line.trim_end()).unwrap() else {
            return;
        };
        seen.lock().unwrap().push(*request);
        let frame = json!({"frame": "response", "id": id, "envelope": envelope});
        writeln!(writer, "{frame}").unwrap();
    }
}

fn ok_envelope() -> Response {
    Response::ok("steer", Some(json!({"sent": true})))
}

fn fallback() -> crate::http::Handler {
    Arc::new(|_| HttpResponse::text("200 OK", "fallback").into())
}

/// Starts the web front over a fake controller answering `envelope`.
fn front(dir: &tempfile::TempDir, envelope: &Response) -> (Server, Seen) {
    let sock = dir.path().join("c.sock");
    let seen = serve(&sock, envelope);
    let web = start(0, TOKEN.to_owned(), handler(Client::new(sock), fallback())).unwrap();
    (web, seen)
}

/// Sends one raw request with the credentials of a browser action and returns the reply.
fn send(web: &Server, method: &str, path: &str, body: &str) -> String {
    let port = web.addr().port();
    let mut s = TcpStream::connect(web.addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
         Authorization: Bearer {TOKEN}\r\nX-Moor-Csrf: 1\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out
}

fn post(web: &Server, path: &str, body: &Value) -> String {
    send(web, "POST", path, &body.to_string())
}

/// The JSON body of a response.
fn body(resp: &str) -> Value {
    let (_, text) = resp.split_once("\r\n\r\n").unwrap();
    serde_json::from_str(text).unwrap()
}

fn name(s: &str) -> BoxName {
    BoxName::parse_allow_bare(s).unwrap()
}

fn steer(text: Option<&str>, menu: Option<u32>) -> Request {
    Request::Steer(SteerReq {
        task: name("t"),
        text: text.map(str::to_owned),
        menu,
    })
}

#[test]
fn each_action_makes_one_request_and_returns_the_envelope() {
    let cases = [
        (
            "/steer",
            json!({"task": "t", "text": "go"}),
            steer(Some("go"), None),
        ),
        (
            "/answer",
            json!({"task": "t", "option": 2}),
            steer(None, Some(2)),
        ),
        (
            "/end-session",
            json!({"task": "t"}),
            Request::EndSession(EndSessionReq { task: "t".into() }),
        ),
        (
            "/close",
            json!({"task": "t"}),
            Request::Close(CloseReq {
                task: name("t"),
                force: false,
                trust_moor_yaml: false,
            }),
        ),
    ];
    for (path, payload, expected) in cases {
        let dir = tempfile::tempdir().unwrap();
        let (web, seen) = front(&dir, &ok_envelope());
        let resp = post(&web, path, &payload);
        assert!(resp.starts_with("HTTP/1.1 200"), "{path}: {resp}");
        assert!(resp.contains("application/json"), "{path}: {resp}");
        assert_eq!(body(&resp), json!(ok_envelope()), "{path}");
        assert_eq!(*seen.lock().unwrap(), vec![expected], "{path}");
    }
}

#[test]
fn refusal_envelope_passes_through_unchanged() {
    let refusal = Response::err(
        "close",
        ErrorView {
            kind: ErrorKind::Refused,
            message: "box has unfinished work".into(),
            hints: vec!["moor close --force".into()],
            context: vec![],
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let (web, seen) = front(&dir, &refusal);
    let resp = post(&web, "/close", &json!({"task": "t"}));
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let envelope = body(&resp);
    assert_eq!(envelope["class"], "refused");
    assert_eq!(envelope["error"]["kind"], "refused");
    assert_eq!(envelope["error"]["message"], "box has unfinished work");
    assert_eq!(envelope["error"]["hints"], json!(["moor close --force"]));
    assert_eq!(envelope, json!(refusal));
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[test]
fn malformed_body_is_400_without_a_socket_request() {
    let dir = tempfile::tempdir().unwrap();
    let (web, seen) = front(&dir, &ok_envelope());
    for (path, raw) in [
        ("/steer", "not json"),
        ("/steer", r#"{"task":"t"}"#),
        ("/answer", r#"{"task":"t"}"#),
        ("/close", r#"{"task":"t","extra":1}"#),
    ] {
        let resp = send(&web, "POST", path, raw);
        assert!(resp.starts_with("HTTP/1.1 400"), "{path} {raw}: {resp}");
    }
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn unreachable_controller_is_502() {
    let dir = tempfile::tempdir().unwrap();
    let client = Client::new(dir.path().join("missing.sock"));
    let web = start(0, TOKEN.to_owned(), handler(client, fallback())).unwrap();
    let resp = post(&web, "/close", &json!({"task": "t"}));
    assert!(resp.starts_with("HTTP/1.1 502"), "{resp}");
}

/// Serves hello, reads one request, then writes `tail` and hangs up without a response.
fn serve_then_drop(socket: &Path, tail: &'static [u8]) -> Seen {
    let listener = UnixListener::bind(socket).unwrap();
    let seen = Seen::default();
    let record = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let record = record.clone();
            std::thread::spawn(move || {
                let stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut writer = stream;
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                writeln!(writer, r#"{{"frame":"hello","protocol":{PROTOCOL}}}"#).unwrap();
                line.clear();
                reader.read_line(&mut line).unwrap();
                if let ClientFrame::Request { request, .. } = parse_frame(line.trim_end()).unwrap()
                {
                    record.lock().unwrap().push(*request);
                }
                writer.write_all(tail).unwrap();
            });
        }
    });
    seen
}

#[test]
fn mid_exchange_io_error_is_502() {
    for tail in [&b""[..], &br#"{"frame":"response","id":"#[..]] {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("c.sock");
        let seen = serve_then_drop(&sock, tail);
        let web = start(0, TOKEN.to_owned(), handler(Client::new(sock), fallback())).unwrap();
        let resp = post(&web, "/close", &json!({"task": "t"}));
        assert!(resp.starts_with("HTTP/1.1 502"), "{tail:?}: {resp}");
        assert_eq!(seen.lock().unwrap().len(), 1, "{tail:?}");
    }
}

#[test]
fn other_requests_reach_the_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let (web, seen) = front(&dir, &ok_envelope());
    let get = send(&web, "GET", "/close", "");
    assert!(
        get.starts_with("HTTP/1.1 200") && get.ends_with("fallback"),
        "{get}"
    );
    let unknown = post(&web, "/nope", &json!({"task": "t"}));
    assert!(unknown.ends_with("fallback"), "{unknown}");
    assert!(seen.lock().unwrap().is_empty());
}
