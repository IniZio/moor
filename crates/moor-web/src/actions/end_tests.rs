#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "tests assert on fixed fixtures"
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::controller::{parse_frame, ClientFrame, PROTOCOL};
use moor_core::request::{CloseReq, EndHostSessionReq, Request, SessionsReq};
use moor_core::response::{ErrorKind, ErrorView, Response};
use moor_core::{BoxName, SessionId};
use serde_json::{json, Value};

use super::handler;
use crate::actions::start::StartCtx;
use crate::client::Client;
use crate::db::{Db, RequestRow};
use crate::http::{start, HttpResponse, Server};
use crate::jobs::Jobs;

const TOKEN: &str = "t0ken";
const KEY: &str = "k1";
const ROOT: i64 = 7;

/// Requests the fake controller received, in arrival order.
type Seen = Arc<Mutex<Vec<Request>>>;

/// The live Sessions and which calls the fake controller refuses.
#[derive(Clone, Default)]
struct Script {
    /// `(parent, session, box)` of every live non-root Session.
    tree: Vec<(i64, i64, Option<&'static str>)>,
    /// Box whose close is refused.
    refuse_close: Option<&'static str>,
    /// Session whose end is refused.
    refuse_end: Option<i64>,
}

fn refused(cmd: &str, message: &str) -> Response {
    Response::err(
        cmd,
        ErrorView {
            kind: ErrorKind::Refused,
            message: message.into(),
            hints: vec![],
            context: vec![],
        },
    )
}

fn serve(socket: &Path, script: &Script) -> Seen {
    let listener = UnixListener::bind(socket).unwrap();
    let seen = Seen::default();
    let (record, script) = (seen.clone(), script.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let (record, script) = (record.clone(), script.clone());
            std::thread::spawn(move || converse(&record, &script, stream.unwrap()));
        }
    });
    seen
}

fn answer(script: &Script, request: &Request) -> Response {
    match request {
        Request::Sessions(SessionsReq { parent }) => {
            let rows: Vec<Value> = script
                .tree
                .iter()
                .filter(|(p, _, _)| Some(SessionId::new(*p)) == *parent)
                .map(|(p, s, b)| json!({"session": s, "box": b, "parent": p}))
                .collect();
            Response::ok("sessions", Some(json!({"sessions": rows})))
        }
        Request::Close(c) if Some(c.task.as_str()) == script.refuse_close => {
            refused("close", "has unmerged work")
        }
        Request::EndHostSession(e) if Some(e.session.get()) == script.refuse_end => {
            refused("end-host-session", "still busy")
        }
        _ => Response::ok("done", Some(json!({}))),
    }
}

fn converse(seen: &Seen, script: &Script, stream: UnixStream) {
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
        let envelope = answer(script, &request);
        seen.lock().unwrap().push(*request);
        let frame = json!({"frame": "response", "id": id, "envelope": envelope});
        writeln!(writer, "{frame}").unwrap();
    }
}

/// A started front with its collaborators.
struct Front {
    web: Server,
    seen: Seen,
    jobs: Jobs,
    db: Arc<Mutex<Db>>,
    _dir: tempfile::TempDir,
}

fn clock() -> i64 {
    1_700_000_500
}

fn front(script: &Script, ended_at: Option<i64>) -> Front {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let seen = serve(&sock, script);
    let db = Db::open(&dir.path().join("web.db")).unwrap();
    db.insert_request(&RequestRow {
        key: KEY.to_owned(),
        root_session: SessionId::new(ROOT),
        repo: "r".to_owned(),
        created_at: 1_700_000_000,
        ended_at,
    })
    .unwrap();
    let db = Arc::new(Mutex::new(db));
    let jobs = Jobs::new();
    let ctx = StartCtx {
        client: Client::new(sock),
        db: db.clone(),
        jobs: jobs.clone(),
        state_dir: dir.path().to_path_buf(),
        clock,
    };
    let fallback: crate::http::Handler =
        Arc::new(|_| HttpResponse::text("200 OK", "fallback").into());
    let web = start(0, TOKEN.to_owned(), handler(ctx, fallback)).unwrap();
    Front {
        web,
        seen,
        jobs,
        db,
        _dir: dir,
    }
}

fn post(web: &Server, path: &str) -> String {
    let port = web.addr().port();
    let mut s = TcpStream::connect(web.addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
         Authorization: Bearer {TOKEN}\r\nX-Moor-Csrf: 1\r\n\
         Content-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}"
    )
    .unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out
}

/// Ends the request and returns the finished job.
fn end_request(f: &Front) -> Value {
    let resp = post(&f.web, &format!("/requests/{KEY}/end"));
    assert!(resp.starts_with("HTTP/1.1 202"), "{resp}");
    let (_, text) = resp.split_once("\r\n\r\n").unwrap();
    let accepted: Value = serde_json::from_str(text).unwrap();
    assert_eq!(accepted["key"], KEY);
    let job = accepted["job"].as_str().unwrap();
    for _ in 0..1000 {
        let snap = f.jobs.snapshot(job).unwrap();
        if snap["state"] != "running" {
            return snap;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("job still running");
}

fn close(task: &str) -> Request {
    Request::Close(CloseReq {
        task: BoxName::parse_allow_bare(task).unwrap(),
        force: false,
        trust_moor_yaml: false,
    })
}

fn end_session(session: i64) -> Request {
    Request::EndHostSession(EndHostSessionReq {
        session: SessionId::new(session),
    })
}

/// Only the calls that change something.
fn mutations(f: &Front) -> Vec<Request> {
    let seen = f.seen.lock().unwrap().clone();
    seen.into_iter()
        .filter(|r| !matches!(r, Request::Sessions(_)))
        .collect()
}

fn ended_at(f: &Front) -> Option<i64> {
    f.db.lock().unwrap().request(KEY).unwrap().unwrap().ended_at
}

fn tree() -> Script {
    Script {
        tree: vec![(ROOT, 10, Some("a")), (ROOT, 11, Some("b")), (10, 12, None)],
        ..Script::default()
    }
}

#[test]
fn ends_children_deepest_first_then_the_root() {
    let f = front(&tree(), None);
    let job = end_request(&f);
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(
        mutations(&f),
        vec![end_session(12), close("a"), close("b"), end_session(ROOT)]
    );
    assert_eq!(ended_at(&f), Some(1_700_000_500));
}

#[test]
fn the_result_lists_each_outcome() {
    let f = front(&tree(), None);
    let job = end_request(&f);
    assert_eq!(job["result"]["key"], KEY);
    assert_eq!(job["result"]["root"], ROOT);
    assert_eq!(job["result"]["root_ended"], true);
    let closed = job["result"]["closed"].as_array().unwrap();
    assert_eq!(closed.len(), 3);
    assert!(closed.iter().all(|o| o["ok"] == true));
    assert_eq!(closed[0]["session"], 12);
    assert_eq!(closed[1]["box"], "a");
}

#[test]
fn refused_close_keeps_the_root_and_closes_the_rest() {
    let script = Script {
        refuse_close: Some("a"),
        ..tree()
    };
    let f = front(&script, None);
    let job = end_request(&f);
    assert_eq!(job["state"], "failed", "{job}");
    assert_eq!(job["failure"]["step"], "close-children");
    let message = job["failure"]["message"].as_str().unwrap();
    assert!(
        message.starts_with("1 of 3 could not be closed: a: "),
        "{message}"
    );
    assert!(message.contains("has unmerged work"), "{message}");
    assert_eq!(mutations(&f), vec![end_session(12), close("a"), close("b")]);
    assert_eq!(ended_at(&f), None);
}

#[test]
fn refused_root_end_fails_the_end_root_step() {
    let script = Script {
        refuse_end: Some(ROOT),
        ..tree()
    };
    let f = front(&script, None);
    let job = end_request(&f);
    assert_eq!(job["state"], "failed", "{job}");
    assert_eq!(job["failure"]["step"], "end-root");
    assert!(job["failure"]["message"]
        .as_str()
        .unwrap()
        .contains("still busy"));
    assert_eq!(ended_at(&f), None);
}

#[test]
fn a_box_shared_by_two_children_is_closed_once() {
    let script = Script {
        tree: vec![(ROOT, 10, Some("a")), (ROOT, 11, Some("a"))],
        ..Script::default()
    };
    let f = front(&script, None);
    let job = end_request(&f);
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(mutations(&f), vec![close("a"), end_session(ROOT)]);
}

#[test]
fn unknown_key_is_404() {
    let f = front(&tree(), None);
    let resp = post(&f.web, "/requests/nope/end");
    assert!(resp.starts_with("HTTP/1.1 404"), "{resp}");
    assert!(f.seen.lock().unwrap().is_empty());
}

#[test]
fn ended_request_is_409_without_a_socket_call() {
    let f = front(&tree(), Some(1_700_000_100));
    let resp = post(&f.web, &format!("/requests/{KEY}/end"));
    assert!(resp.starts_with("HTTP/1.1 409"), "{resp}");
    assert!(f.seen.lock().unwrap().is_empty());
}

#[test]
fn other_paths_reach_the_fallback() {
    let f = front(&tree(), None);
    let resp = post(&f.web, "/requests");
    assert!(resp.ends_with("fallback"), "{resp}");
}
