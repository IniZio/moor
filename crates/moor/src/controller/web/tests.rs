//! Tests for the web server, using a fake feed and a raw TCP client.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::controller::ServerFrame;
use moor_core::domain::task::TaskState;
use moor_core::view::TaskView;

use super::{start, Feed, FeedFactory, Web};

/// Token the test server requires.
const TOKEN: &str = "s3cret-token";

/// Shared, appendable event list.
type Events = Arc<Mutex<Vec<ServerFrame>>>;

/// Feed over fixed tasks and a shared event list.
struct Fake {
    /// Events served.
    events: Events,
}

impl Feed for Fake {
    fn tasks(&self) -> Result<Vec<TaskView>, String> {
        Ok(vec![task("alpha"), task("beta")])
    }

    fn events_after(&self, after: i64, limit: usize) -> Result<Vec<ServerFrame>, String> {
        let all = self.events.lock().map_err(|e| e.to_string())?;
        Ok(all
            .iter()
            .filter(|f| matches!(f, ServerFrame::Event { cursor, .. } if *cursor > after))
            .take(limit)
            .cloned()
            .collect())
    }
}

/// A minimal Task row named `name`.
fn task(name: &str) -> TaskView {
    TaskView {
        id: 1,
        name: name.into(),
        repo: "r".into(),
        issue: None,
        goal: None,
        state: TaskState::Working,
        state_reason: None,
        base: None,
        branch: None,
        worktree: None,
        workspace: None,
        r#box: None,
        agent: None,
        run: None,
        updated_at: "2026-01-01T00:00:00Z".into(),
    }
}

/// An event with the given cursor.
fn event(cursor: i64) -> ServerFrame {
    ServerFrame::Event {
        cursor,
        task: "alpha".into(),
        kind: "k".into(),
        payload: serde_json::json!({}),
        ts: 0,
    }
}

/// Starts a server over a fake feed holding `seed` events.
fn server(seed: Vec<ServerFrame>) -> (Web, Events) {
    let events: Events = Arc::new(Mutex::new(seed));
    let shared = Arc::clone(&events);
    let feed: FeedFactory = Arc::new(move || {
        Ok(Box::new(Fake {
            events: Arc::clone(&shared),
        }) as Box<dyn Feed>)
    });
    let web = start(0, TOKEN.into(), feed, Duration::from_millis(10)).unwrap();
    (web, events)
}

/// Sends a request head and returns the open stream.
fn open(web: &Web, path: &str, headers: &[&str]) -> TcpStream {
    let mut s = TcpStream::connect(web.addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\n").unwrap();
    for h in headers {
        write!(s, "{h}\r\n").unwrap();
    }
    write!(s, "\r\n").unwrap();
    s
}

/// Fetches a whole response (the server closes after it).
fn fetch(web: &Web, path: &str, headers: &[&str]) -> String {
    let mut out = String::new();
    open(web, path, headers).read_to_string(&mut out).unwrap();
    out
}

/// Cookie header carrying the valid token.
fn cookie() -> String {
    format!("Cookie: moor_token={TOKEN}")
}

#[test]
fn missing_or_wrong_token_is_unauthorized_everywhere() {
    let (web, _) = server(vec![event(1)]);
    for path in ["/", "/tasks", "/events", "/app.js"] {
        assert!(fetch(&web, path, &[]).starts_with("HTTP/1.1 401"), "{path}");
        let wrong = fetch(
            &web,
            path,
            &["Authorization: Bearer nope", "Cookie: moor_token=nope"],
        );
        assert!(wrong.starts_with("HTTP/1.1 401"), "{path}");
        assert!(wrong.ends_with("unauthorized"));
    }
    let query_off_page = fetch(&web, &format!("/tasks?token={TOKEN}"), &[]);
    assert!(query_off_page.starts_with("HTTP/1.1 401"));
}

#[test]
fn binds_loopback_only() {
    let (web, _) = server(vec![]);
    assert!(web.addr().ip().is_loopback());
}

#[test]
fn tasks_and_page_are_served_with_credentials() {
    let (web, _) = server(vec![]);
    let bearer = format!("Authorization: Bearer {TOKEN}");
    let tasks = fetch(&web, "/tasks", &[&bearer]);
    assert!(tasks.starts_with("HTTP/1.1 200"));
    assert!(tasks.contains("application/json"));
    assert!(tasks.contains("\"alpha\"") && tasks.contains("\"beta\""));
    assert!(tasks.contains("{\"tasks\":["));
    assert!(tasks.contains("X-Content-Type-Options: nosniff"));
    assert!(tasks.contains("Content-Security-Policy: default-src 'none'"));
}

#[test]
fn query_token_on_page_sets_cookie() {
    let (web, _) = server(vec![]);
    let page = fetch(&web, &format!("/?token={TOKEN}"), &[]);
    assert!(page.starts_with("HTTP/1.1 200"));
    assert!(page.contains(&format!(
        "Set-Cookie: moor_token={TOKEN}; HttpOnly; SameSite=Strict; Path=/"
    )));
    assert!(page.ends_with(include_str!("page.html")));
    assert!(fetch(&web, "/app.js", &[&cookie()]).contains("application/javascript"));
}

#[test]
fn other_methods_and_paths_are_rejected_after_auth() {
    let (web, _) = server(vec![]);
    let mut s = TcpStream::connect(web.addr()).unwrap();
    write!(
        s,
        "POST /tasks HTTP/1.1\r\nCookie: moor_token={TOKEN}\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    assert!(out.starts_with("HTTP/1.1 405"));
    assert!(fetch(&web, "/nope", &[&cookie()]).starts_with("HTTP/1.1 404"));
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

#[test]
fn stream_delivers_new_events_on_the_open_connection() {
    let (web, events) = server(vec![event(1)]);
    let mut r = BufReader::new(open(&web, "/events", &[&cookie()]));
    assert_eq!(read_until(&mut r, "id:"), "id: 1\n");
    events.lock().unwrap().push(event(2));
    assert_eq!(read_until(&mut r, "id:"), "id: 2\n");
    assert!(read_until(&mut r, "data:").contains("\"cursor\":2"));
}

#[test]
fn last_event_id_resumes_after_that_event() {
    let (web, _) = server(vec![event(1), event(2), event(3)]);
    let id = "Last-Event-ID: 2";
    let mut r = BufReader::new(open(&web, "/events", &[&cookie(), id]));
    assert_eq!(read_until(&mut r, "id:"), "id: 3\n");
    let mut r = BufReader::new(open(&web, "/events?since=1", &[&cookie()]));
    assert_eq!(read_until(&mut r, "id:"), "id: 2\n");
}
