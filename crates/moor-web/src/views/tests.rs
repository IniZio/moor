//! Tests for the read endpoints, over a fake controller socket.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "tests assert on fixed fixtures"
)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::controller::{parse_frame, ClientFrame, PROTOCOL};
use moor_core::name::SessionId;
use moor_core::response::{ErrorKind, ErrorView, Response};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::handler;
use crate::client::Client;
use crate::db::{Db, RequestRow};
use crate::http::{start, HttpResponse, Server};

pub(super) const TOKEN: &str = "t0ken";

/// Command names the fake controller received, in order.
pub(super) type Seen = Arc<Mutex<Vec<String>>>;

/// Answers by command name.
pub(super) type Answers = Arc<HashMap<&'static str, Response>>;

/// Answers a request, given as its wire JSON.
pub(super) type Responder = Arc<dyn Fn(&Value) -> Response + Send + Sync>;

/// Requests the fake controller received, as wire JSON, in order.
pub(super) type Sent = Arc<Mutex<Vec<Value>>>;

/// The same answer for every request of a command; an unknown command is a usage error.
pub(super) fn by_command(answers: Answers) -> Responder {
    Arc::new(move |req| {
        let cmd = req["cmd"].as_str().unwrap_or_default();
        answers
            .get(cmd)
            .cloned()
            .unwrap_or_else(|| failure(cmd, ErrorKind::Usage))
    })
}

fn serve_with(socket: &Path, respond: Responder) -> (Seen, Sent) {
    let listener = UnixListener::bind(socket).unwrap();
    let (seen, sent) = (Seen::default(), Sent::default());
    let (record, log) = (seen.clone(), sent.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let (record, log, respond) = (record.clone(), log.clone(), respond.clone());
            std::thread::spawn(move || converse(&record, &log, &respond, stream.unwrap()));
        }
    });
    (seen, sent)
}

fn converse(seen: &Seen, sent: &Sent, respond: &Responder, stream: UnixStream) {
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
        seen.lock().unwrap().push(request.cmd().to_owned());
        let wire = serde_json::to_value(&*request).unwrap();
        let envelope = respond(&wire);
        sent.lock().unwrap().push(wire);
        let frame = json!({"frame": "response", "id": id, "envelope": envelope});
        writeln!(writer, "{frame}").unwrap();
    }
}

pub(super) fn failure(cmd: &str, kind: ErrorKind) -> Response {
    let error = ErrorView {
        kind,
        message: "no".to_owned(),
        hints: Vec::new(),
        context: Vec::new(),
    };
    Response::err(cmd, error)
}

fn session(id: i64, parent: Option<i64>, bx: Option<&str>, ordinal: i64) -> Value {
    json!({"session": id, "box": bx, "parent": parent, "state": "active",
           "workdir": bx.map(|b| format!("/w/{b}")), "ordinal": ordinal})
}

fn task(name: &str, state: &str, question: Option<&str>) -> Value {
    json!({
        "name": name, "repo": "app", "branch": format!("moor/{name}"), "lifecycle": "ready", "session_state": "working",
        "goal": format!("goal of {name}"),
        "driver": {"state": state},
        "agent": {"state": if question.is_some() { "needs_input" } else { "working" },
                  "question": question, "options": if question.is_some() { json!(["yes", "no"]) } else { json!([]) }},
    })
}

pub(super) fn answers() -> Answers {
    let sessions = json!({"sessions": [
        session(3, Some(1), Some("app/2"), 2),
        session(1, None, None, 0),
        session(2, Some(1), Some("app/1"), 1),
    ]});
    let show = json!({
        "tasks": [task("app/1", "awake", None), task("app/2", "asleep", Some("ship it?"))],
        "task": task("app/1", "awake", None),
        "events": [{"id": 1, "kind": "note", "payload": {}, "ts": 5}],
    });
    let repos = json!({"repos": [{"name": "app", "url": "https://example.test/app.git"}]});
    let note = b"hello";
    Arc::new(HashMap::from([
        ("sessions", Response::ok("sessions", Some(sessions))),
        ("show", Response::ok("show", Some(show))),
        ("repos", Response::ok("repos", Some(repos))),
        (
            "artifacts",
            Response::ok(
                "artifacts",
                Some(json!([record("note.txt", "text/plain", 1, note)])),
            ),
        ),
        (
            "artifact-read",
            Response::ok("artifact-read", Some(chunk(note, 0, 512, &sha(note)))),
        ),
    ]))
}

pub(super) fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn record(name: &str, content_type: &str, version: u32, bytes: &[u8]) -> Value {
    json!({"name": name, "version": version, "kind": "file", "content_type": content_type,
           "size": bytes.len(), "sha256": sha(bytes), "created_at": "2026-01-01T00:00:00Z"})
}

fn b64(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for group in bytes.chunks(3) {
        let mut n = 0u32;
        for i in 0..3 {
            n = (n << 8) | u32::from(group.get(i).copied().unwrap_or(0));
        }
        for i in 0..4 {
            if i <= group.len() {
                let six = usize::try_from((n >> (18 - 6 * i)) & 63).unwrap();
                out.push(char::from(DIGITS[six]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The `artifact-read` value for `len` bytes of `file` from `offset`, claiming hash `claimed`.
pub(super) fn chunk(file: &[u8], offset: usize, len: usize, claimed: &str) -> Value {
    let start = offset.min(file.len());
    let end = (start + len).min(file.len());
    json!({"data_b64": b64(&file[start..end]), "size": file.len(), "sha256": claimed,
           "eof": end == file.len()})
}

fn row(key: &str, session: i64, ended: Option<i64>) -> RequestRow {
    RequestRow {
        key: key.to_owned(),
        root_session: SessionId::new(session),
        repo: "app".to_owned(),
        created_at: 10,
        ended_at: ended,
    }
}

fn fallback() -> crate::http::Handler {
    Arc::new(|_| HttpResponse::text("200 OK", "fallback").into())
}

/// Starts the front over a fake controller; the socket directory outlives the test body.
pub(super) fn front(
    dir: &tempfile::TempDir,
    answers: Answers,
    rows: &[RequestRow],
) -> (Server, Seen) {
    let (web, seen, _) = front_with(dir, by_command(answers), rows);
    (web, seen)
}

/// Like `front`, with answers computed from each request.
pub(super) fn front_with(
    dir: &tempfile::TempDir,
    respond: Responder,
    rows: &[RequestRow],
) -> (Server, Seen, Sent) {
    let sock = dir.path().join("c.sock");
    let (seen, sent) = serve_with(&sock, respond);
    let db = Db::open(&dir.path().join("web.db")).unwrap();
    for r in rows {
        db.insert_request(r).unwrap();
    }
    let handler = handler(Client::new(sock), Arc::new(Mutex::new(db)), fallback());
    (start(0, TOKEN.to_owned(), handler).unwrap(), seen, sent)
}

pub(super) fn get(web: &Server, path: &str) -> String {
    send(web.addr(), "GET", path)
}

fn send(addr: std::net::SocketAddr, method: &str, path: &str) -> String {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\
         Authorization: Bearer {TOKEN}\r\nX-Moor-Csrf: 1\r\nContent-Length: 0\r\n\r\n",
        addr.port()
    )
    .unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out
}

pub(super) fn body(resp: &str) -> Value {
    let (_, text) = resp.split_once("\r\n\r\n").unwrap();
    serde_json::from_str(text).unwrap()
}

#[test]
fn tree_nests_children_with_box_and_agent_state() {
    let dir = tempfile::tempdir().unwrap();
    let (web, _) = front(&dir, answers(), &[]);
    let resp = get(&web, "/tree");
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let roots = body(&resp)["roots"].clone();
    assert_eq!(roots.as_array().unwrap().len(), 1);
    assert_eq!(roots[0]["session"], 1);
    assert_eq!(roots[0]["box"], Value::Null);
    assert_eq!(roots[0]["task"], Value::Null);
    let ids: Vec<&Value> = roots[0]["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| &k["session"])
        .collect();
    assert_eq!(ids, [2, 3]);
}

#[test]
fn child_carries_task_and_agent_question() {
    let dir = tempfile::tempdir().unwrap();
    let (web, _) = front(&dir, answers(), &[]);
    let second = body(&get(&web, "/tree"))["roots"][0]["children"][1].clone();
    assert_eq!(second["parent"], 1);
    assert_eq!(second["workdir"], "/w/app/2");
    assert_eq!(
        second["task"],
        json!({"name": "app/2", "repo": "app", "repo_url": "https://example.test/app.git",
               "branch": "moor/app/2", "lifecycle": "ready", "session_state": "working", "goal": "goal of app/2",
               "box_state": "asleep"})
    );
    assert_eq!(
        second["agent"],
        json!({"state": "needs_input", "question": "ship it?", "options": ["yes", "no"]})
    );
    assert_eq!(second["children"], json!([]));
}

#[test]
fn awake_child_has_no_question() {
    let dir = tempfile::tempdir().unwrap();
    let (web, _) = front(&dir, answers(), &[]);
    let kid = body(&get(&web, "/tree"))["roots"][0]["children"][0].clone();
    assert_eq!(kid["task"]["box_state"], "awake");
    assert_eq!(kid["agent"]["question"], Value::Null);
}

#[test]
fn request_row_attaches_to_its_root() {
    let dir = tempfile::tempdir().unwrap();
    let (web, _) = front(&dir, answers(), &[row("k1", 1, None)]);
    let roots = body(&get(&web, "/tree"))["roots"].clone();
    assert_eq!(
        roots[0]["request"],
        json!({"key": "k1", "repo": "app", "created_at": 10, "ended_at": null})
    );
    assert_eq!(roots[0]["children"][0]["request"], Value::Null);
}

#[test]
fn ended_root_not_live_is_listed_as_gone() {
    let dir = tempfile::tempdir().unwrap();
    let (web, _) = front(
        &dir,
        answers(),
        &[row("k1", 1, None), row("k2", 9, Some(20))],
    );
    let roots = body(&get(&web, "/tree"))["roots"].clone();
    let roots = roots.as_array().unwrap();
    assert_eq!(roots.len(), 2);
    let gone = &roots[1];
    assert_eq!(gone["session"], 9);
    assert_eq!(gone["state"], "gone");
    assert_eq!(gone["request"]["ended_at"], 20);
    assert_eq!(gone["children"], json!([]));
}

#[test]
fn cycles_do_not_loop() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = json!({"sessions": [
        session(1, None, None, 0),
        session(2, Some(1), None, 0),
        session(1, Some(2), None, 0),
    ]});
    let mut map = (*answers()).clone();
    map.insert("sessions", Response::ok("sessions", Some(sessions)));
    let (web, _) = front(&dir, Arc::new(map), &[]);
    let resp = get(&web, "/tree");
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
}

#[test]
fn reads_send_only_commands_that_do_not_wake() {
    let dir = tempfile::tempdir().unwrap();
    let (web, seen) = front(&dir, answers(), &[]);
    let paths = [
        "/tree",
        "/repos",
        "/boxes/x",
        "/boxes/app/1/artifacts",
        "/boxes/app/1/artifacts/note.txt",
    ];
    for path in paths {
        assert!(get(&web, path).starts_with("HTTP/1.1 200"), "{path}");
    }
    let seen = seen.lock().unwrap().clone();
    assert!(seen.len() >= 7, "{seen:?}");
    assert!(seen.iter().any(|c| c == "artifact-read"), "{seen:?}");
    let allowed = [
        "sessions",
        "show",
        "repos",
        "show-session",
        "artifacts",
        "artifact-read",
    ];
    assert!(
        seen.iter().all(|c| allowed.contains(&c.as_str())),
        "{seen:?}"
    );
}

#[test]
fn repos_are_listed() {
    let dir = tempfile::tempdir().unwrap();
    let (web, _) = front(&dir, answers(), &[]);
    let resp = get(&web, "/repos");
    assert_eq!(
        body(&resp),
        json!({"repos": [{"name": "app", "url": "https://example.test/app.git"}]})
    );
}

#[test]
fn box_detail_returns_task_and_events() {
    let dir = tempfile::tempdir().unwrap();
    let (web, seen) = front(&dir, answers(), &[]);
    let v = body(&get(&web, "/boxes/app/1"));
    assert_eq!(v["task"]["name"], "app/1");
    assert_eq!(v["events"][0]["kind"], "note");
    assert_eq!(*seen.lock().unwrap(), ["show"]);
}

#[test]
fn bad_box_name_is_400() {
    let dir = tempfile::tempdir().unwrap();
    let (web, seen) = front(&dir, answers(), &[]);
    assert!(get(&web, "/boxes/Bad%20Name").starts_with("HTTP/1.1 400"));
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn refused_envelope_is_502_with_the_envelope() {
    let dir = tempfile::tempdir().unwrap();
    let refusal = failure("show", ErrorKind::Refused);
    let map = HashMap::from([("show", refusal)]);
    let (web, _) = front(&dir, Arc::new(map), &[]);
    let resp = get(&web, "/boxes/x");
    assert!(resp.starts_with("HTTP/1.1 502"), "{resp}");
    assert_eq!(body(&resp)["class"], "refused");
}

#[test]
fn unreachable_controller_is_502() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("web.db")).unwrap();
    let client = Client::new(dir.path().join("missing.sock"));
    let h = handler(client, Arc::new(Mutex::new(db)), fallback());
    let web = start(0, TOKEN.to_owned(), h).unwrap();
    for path in ["/tree", "/repos", "/boxes/x"] {
        assert!(get(&web, path).starts_with("HTTP/1.1 502"), "{path}");
    }
}

#[test]
fn other_paths_fall_back() {
    let dir = tempfile::tempdir().unwrap();
    let (web, seen) = front(&dir, answers(), &[]);
    assert!(get(&web, "/other").ends_with("fallback"));
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn a_worker_whose_session_ended_but_whose_box_is_open_stays_in_the_tree() {
    let dir = tempfile::tempdir().unwrap();
    let mut open = task("app/3", "awake", None);
    open["id"] = json!(5);
    let mut closed = task("app/3", "gone", None);
    closed["id"] = json!(6);
    closed["lifecycle"] = json!("closed");
    let mut map = (*answers()).clone();
    map.insert(
        "sessions",
        Response::ok(
            "sessions",
            Some(json!({"sessions": [session(1, None, None, 0)]})),
        ),
    );
    map.insert(
        "show",
        Response::ok("show", Some(json!({"tasks": [closed, open]}))),
    );
    let kids = json!({"children": [
        {"session": 4, "box": "app/3", "box_id": 5, "state": "ended"},
        {"session": 3, "box": "app/3", "box_id": 6, "state": "ended"},
        {"session": 2, "box": null, "state": "ended"},
    ]});
    map.insert("show-session", Response::ok("show-session", Some(kids)));
    let (web, _) = front(&dir, Arc::new(map), &[row("k1", 1, None)]);
    let roots = body(&get(&web, "/tree"))["roots"].clone();
    let kids = roots[0]["children"].as_array().unwrap();
    assert_eq!(kids.len(), 1, "{kids:?}");
    assert_eq!(kids[0]["session"], 4);
    assert_eq!(kids[0]["state"], "ended");
    assert_eq!(kids[0]["box"], "app/3");
    assert_eq!(kids[0]["task"]["box_state"], "awake");
}

#[test]
fn a_gone_root_lists_its_ended_workers_whose_box_is_open() {
    let dir = tempfile::tempdir().unwrap();
    let mut open = task("app/3", "awake", None);
    open["id"] = json!(5);
    let mut map = (*answers()).clone();
    map.insert(
        "sessions",
        Response::ok("sessions", Some(json!({"sessions": []}))),
    );
    map.insert("show", Response::ok("show", Some(json!({"tasks": [open]}))));
    let kids = json!({"children": [
        {"session": 4, "box": "app/3", "box_id": 5, "state": "ended"},
        {"session": 3, "box": "app/3", "box_id": 6, "state": "ended"},
    ]});
    map.insert("show-session", Response::ok("show-session", Some(kids)));
    let (web, _) = front(&dir, Arc::new(map), &[row("k1", 1, None)]);
    let roots = body(&get(&web, "/tree"))["roots"].clone();
    assert_eq!(roots[0]["state"], "gone");
    let kids = roots[0]["children"].as_array().unwrap();
    assert_eq!(kids.len(), 1, "{kids:?}");
    assert_eq!(kids[0]["session"], 4);
    assert_eq!(kids[0]["task"]["box_state"], "awake");
}
