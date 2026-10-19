#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "tests assert on fixed fixtures"
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::controller::{parse_frame, ClientFrame, PROTOCOL};
use moor_core::request::{
    BindSessionReq, EndHostSessionReq, OpenHostSessionReq, ReposReq, Request, StartReq,
};
use moor_core::response::{ErrorKind, ErrorView, Response};
use moor_core::{BoxName, SessionId};
use serde_json::{json, Value};

use super::{handler, StartCtx};
use crate::client::Client;
use crate::db::{Db, RequestRow};
use crate::http::{start, HttpResponse, Server};
use crate::jobs::Jobs;

const TOKEN: &str = "t0ken";

/// Requests the fake controller received, in arrival order.
type Seen = Arc<Mutex<Vec<Request>>>;

/// What the fake controller answers to the steps that can go wrong.
#[derive(Clone)]
struct Script {
    /// Answer to `repos`.
    repos: Response,
    /// Answer to `bind-session`.
    bind: Response,
    /// Answer to `start`.
    start: Response,
}

fn script() -> Script {
    Script {
        repos: repos(&[("r", "https://example.com/r.git")]),
        bind: Response::ok(
            "bind-session",
            Some(json!({"created": true, "binding": {}})),
        ),
        start: Response::ok("start", Some(json!({"name": "t"}))),
    }
}

fn repos(list: &[(&str, &str)]) -> Response {
    let repos: Vec<Value> = list
        .iter()
        .map(|(name, url)| json!({"name": name, "url": url}))
        .collect();
    Response::ok("repos", Some(json!({ "repos": repos })))
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

/// Serves hello and answers each request by command, recording the requests.
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
        let envelope = match &*request {
            Request::OpenHostSession(_) => Response::ok(
                "open-host-session",
                Some(json!({"session": 7, "workdir": "/w", "state": "open"})),
            ),
            Request::Repos(_) => script.repos.clone(),
            Request::BindSession(_) => script.bind.clone(),
            Request::EndHostSession(_) => Response::ok("end-host-session", Some(json!({}))),
            _ => {
                let note = json!({"frame": "progress", "id": id, "message": "cloning repo"});
                writeln!(writer, "{note}").unwrap();
                script.start.clone()
            }
        };
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
    dir: tempfile::TempDir,
}

fn clock() -> i64 {
    1_700_000_000
}

fn front(script: &Script) -> Front {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let seen = serve(&sock, script);
    let db = Arc::new(Mutex::new(Db::open(&dir.path().join("web.db")).unwrap()));
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
        dir,
    }
}

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

fn body(resp: &str) -> Value {
    let (_, text) = resp.split_once("\r\n\r\n").unwrap();
    serde_json::from_str(text).unwrap()
}

/// Posts a new request and returns its key and the finished job.
fn new_request(f: &Front, payload: &Value) -> (String, Value) {
    let resp = post(&f.web, "/requests", payload);
    assert!(resp.starts_with("HTTP/1.1 202"), "{resp}");
    let accepted = body(&resp);
    let key = accepted["key"].as_str().unwrap().to_owned();
    (key, finished(f, accepted["job"].as_str().unwrap()))
}

/// Polls the job until it is no longer running.
fn finished(f: &Front, job: &str) -> Value {
    for _ in 0..1000 {
        let snap = f.jobs.snapshot(job).unwrap();
        if snap["state"] != "running" {
            return snap;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("job still running");
}

fn name(s: &str) -> BoxName {
    BoxName::parse_allow_bare(s).unwrap()
}

fn start_req(box_name: &str, goal: Option<&str>, repo: &str, parent: i64) -> Request {
    Request::Start(StartReq {
        name: name(box_name),
        goal: goal.map(str::to_owned),
        issue: None,
        worktree: None,
        base: None,
        model: None,
        no_agent: false,
        hold: false,
        resume: false,
        trust_moor_yaml: false,
        repo: Some(repo.to_owned()),
        parent: Some(SessionId::new(parent)),
    })
}

fn payload() -> Value {
    json!({"repo": "r", "name": "t", "goal": "ship it"})
}

#[test]
fn new_request_opens_binds_and_starts_under_the_root() {
    let f = front(&script());
    let (key, job) = new_request(&f, &payload());
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(key.len(), 32);
    let workdir = f.dir.path().join("requests").join(&key);
    let seen = f.seen.lock().unwrap().clone();
    assert_eq!(
        seen,
        vec![
            Request::Repos(ReposReq {}),
            Request::OpenHostSession(OpenHostSessionReq {
                workdir: workdir.to_str().unwrap().to_owned(),
                agent: None,
                agent_session_id: None,
                parent: None,
            }),
            Request::BindSession(BindSessionReq {
                front: "web".into(),
                key: key.clone(),
                session: SessionId::new(7),
                owner: "web".into(),
            }),
            start_req("t", Some("ship it"), "r", 7),
        ]
    );
    assert!(workdir.is_absolute());
    let mode = std::fs::metadata(&workdir).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
    assert_eq!(job["result"]["key"], json!(key));
    assert_eq!(job["result"]["root"], 7);
    assert_eq!(job["result"]["start"], json!({"name": "t"}));
}

#[test]
fn new_request_records_the_request() {
    let f = front(&script());
    let (key, _) = new_request(&f, &payload());
    let row = f.db.lock().unwrap().request(&key).unwrap().unwrap();
    assert_eq!(
        row,
        RequestRow {
            key,
            root_session: SessionId::new(7),
            repo: "r".into(),
            created_at: 1_700_000_000,
            ended_at: None,
        }
    );
}

#[test]
fn empty_goal_sends_none() {
    let f = front(&script());
    let (_, job) = new_request(&f, &json!({"repo": "r", "name": "t", "goal": ""}));
    assert_eq!(job["state"], "done", "{job}");
    let seen = f.seen.lock().unwrap().clone();
    assert_eq!(seen.last().unwrap(), &start_req("t", None, "r", 7));
}

#[test]
fn controller_progress_reaches_the_job() {
    let f = front(&script());
    let (_, job) = new_request(&f, &payload());
    let lines = job["progress"].to_string();
    assert!(lines.contains("cloning repo"), "{lines}");
}

#[test]
fn refused_bind_fails_the_job_and_sends_no_start() {
    let f = front(&Script {
        bind: refused("bind-session", "key taken"),
        ..script()
    });
    let (key, job) = new_request(&f, &payload());
    assert_eq!(job["state"], "failed", "{job}");
    assert_eq!(job["failure"]["step"], "bind-session");
    assert_eq!(job["failure"]["message"], "key taken");
    let seen = f.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 4, "{seen:?}");
    assert_eq!(seen[3], end_root());
    assert!(!seen.iter().any(|r| matches!(r, Request::Start(_))));
    assert!(f.db.lock().unwrap().request(&key).unwrap().is_none());
}

fn end_root() -> Request {
    Request::EndHostSession(EndHostSessionReq {
        session: SessionId::new(7),
    })
}

#[test]
fn failed_save_ends_the_root_and_sends_no_start() {
    let f = front(&script());
    let db = rusqlite::Connection::open(f.dir.path().join("web.db")).unwrap();
    db.execute("DROP TABLE requests", []).unwrap();
    let (_, job) = new_request(&f, &payload());
    assert_eq!(job["state"], "failed", "{job}");
    assert_eq!(job["failure"]["step"], "save-request");
    let seen = f.seen.lock().unwrap().clone();
    assert_eq!(seen.last().unwrap(), &end_root(), "{seen:?}");
    assert!(!seen.iter().any(|r| matches!(r, Request::Start(_))));
}

#[test]
fn unknown_repo_fails_before_anything_is_opened() {
    let f = front(&script());
    let (key, job) = new_request(&f, &json!({"repo": "x", "name": "t", "goal": ""}));
    assert_eq!(job["state"], "failed", "{job}");
    assert_eq!(job["failure"]["step"], "repo");
    assert_eq!(
        job["failure"]["message"],
        "repo x is not known to the controller"
    );
    let seen = f.seen.lock().unwrap().clone();
    assert_eq!(seen, vec![Request::Repos(ReposReq {})]);
    assert!(!f.dir.path().join("requests").join(key).exists());
}

#[test]
fn empty_repo_list_says_none_are_configured() {
    let f = front(&Script {
        repos: repos(&[]),
        ..script()
    });
    let (_, job) = new_request(&f, &payload());
    assert_eq!(job["failure"]["step"], "repo", "{job}");
    assert_eq!(
        job["failure"]["message"],
        "repo r is not known to the controller; no repos are configured"
    );
}

#[test]
fn repo_matching_a_url_is_accepted() {
    let f = front(&script());
    let url = "https://example.com/r.git";
    let (_, job) = new_request(&f, &json!({"repo": url, "name": "t", "goal": ""}));
    assert_eq!(job["state"], "done", "{job}");
}

#[test]
fn failed_start_keeps_the_request_open_for_more_workers() {
    let f = front(&Script {
        start: refused("start", "no such repo"),
        ..script()
    });
    let (key, job) = new_request(&f, &payload());
    assert_eq!(job["failure"]["step"], "start", "{job}");
    let seen = f.seen.lock().unwrap().clone();
    assert!(!seen.contains(&end_root()), "{seen:?}");
    let row = f.db.lock().unwrap().request(&key).unwrap().unwrap();
    assert_eq!(row.ended_at, None);
    let resp = post(
        &f.web,
        &format!("/requests/{key}/workers"),
        &json!({"name": "w2", "goal": ""}),
    );
    assert!(resp.starts_with("HTTP/1.1 202"), "{resp}");
    finished(&f, body(&resp)["job"].as_str().unwrap());
    let seen = f.seen.lock().unwrap().clone();
    assert_eq!(seen.last().unwrap(), &start_req("w2", None, "r", 7));
}

#[test]
fn start_with_failed_step_is_still_done() {
    let f = front(&Script {
        start: Response::ok("start", Some(json!({"failed_step": "setup"}))),
        ..script()
    });
    let (_, job) = new_request(&f, &payload());
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(job["result"]["start"]["failed_step"], "setup");
}

#[test]
fn refused_start_fails_the_job_at_start() {
    let f = front(&Script {
        start: refused("start", "no such repo"),
        ..script()
    });
    let (_, job) = new_request(&f, &payload());
    assert_eq!(job["failure"]["step"], "start", "{job}");
    assert_eq!(job["failure"]["message"], "no such repo");
}

#[test]
fn added_worker_sends_one_start_under_the_root() {
    let f = front(&script());
    let (key, _) = new_request(&f, &payload());
    let before = f.seen.lock().unwrap().len();
    let resp = post(
        &f.web,
        &format!("/requests/{key}/workers"),
        &json!({"name": "w2", "goal": ""}),
    );
    assert!(resp.starts_with("HTTP/1.1 202"), "{resp}");
    let accepted = body(&resp);
    assert_eq!(accepted["key"], json!(key));
    let job = finished(&f, accepted["job"].as_str().unwrap());
    assert_eq!(job["state"], "done", "{job}");
    let seen = f.seen.lock().unwrap().clone();
    assert_eq!(seen[before..], [start_req("w2", None, "r", 7)]);
}

#[test]
fn worker_on_unknown_or_ended_request_is_refused() {
    let f = front(&script());
    let resp = post(
        &f.web,
        "/requests/nope/workers",
        &json!({"name": "w", "goal": ""}),
    );
    assert!(resp.starts_with("HTTP/1.1 404"), "{resp}");
    let (key, _) = new_request(&f, &payload());
    assert!(f.db.lock().unwrap().end_request(&key, 5).unwrap());
    let before = f.seen.lock().unwrap().len();
    let resp = post(
        &f.web,
        &format!("/requests/{key}/workers"),
        &json!({"name": "w", "goal": ""}),
    );
    assert!(resp.starts_with("HTTP/1.1 409"), "{resp}");
    assert_eq!(f.seen.lock().unwrap().len(), before);
}

#[test]
fn bad_bodies_are_400_without_a_socket_request() {
    let f = front(&script());
    for (path, raw) in [
        ("/requests", "not json"),
        ("/requests", r#"{"repo":"r","name":"t"}"#),
        ("/requests", r#"{"repo":"r","name":"t","goal":"","x":1}"#),
        ("/requests", r#"{"repo":"r","name":"Bad Name!","goal":""}"#),
    ] {
        let resp = send(&f.web, "POST", path, raw);
        assert!(resp.starts_with("HTTP/1.1 400"), "{path} {raw}: {resp}");
    }
    let (key, _) = new_request(&f, &payload());
    let before = f.seen.lock().unwrap().len();
    let resp = send(
        &f.web,
        "POST",
        &format!("/requests/{key}/workers"),
        r#"{"name":"w"}"#,
    );
    assert!(resp.starts_with("HTTP/1.1 400"), "{resp}");
    assert_eq!(f.seen.lock().unwrap().len(), before);
}

#[test]
fn other_routes_reach_the_fallback() {
    let f = front(&script());
    let get = send(&f.web, "GET", "/requests", "");
    assert!(get.ends_with("fallback"), "{get}");
    let other = post(&f.web, "/nope", &json!({}));
    assert!(other.ends_with("fallback"), "{other}");
    assert!(f.seen.lock().unwrap().is_empty());
}
