//! Tests for the POST actions: requests sent, envelopes passed through, CSRF and auth.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded JSON"
)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use moor_core::{BoxName, CloseReq, Request, StartReq, SteerReq};
use serde_json::{json, Value};

use super::{post, server_with_calls, Calls, Web, REFUSAL, TOKEN};

const JSON: &str = "Content-Type: application/json";
const CSRF: &str = "X-Moor-Csrf: 1";

fn bearer() -> String {
    format!("Authorization: Bearer {TOKEN}")
}

/// POSTs JSON `body` with valid credentials and CSRF header plus `extra` headers.
fn act(web: &Web, path: &str, extra: &[&str], body: &Value) -> String {
    let auth = bearer();
    let mut headers = vec![auth.as_str(), JSON, CSRF];
    headers.extend_from_slice(extra);
    post(web, path, &headers, &body.to_string())
}

/// The JSON body of a response.
fn body(resp: &str) -> Value {
    let (_, b) = resp.split_once("\r\n\r\n").expect("body");
    serde_json::from_str(b).expect("json body")
}

fn task(name: &str) -> BoxName {
    BoxName::parse_allow_bare(name).unwrap()
}

fn recorded(calls: &Calls) -> Vec<Request> {
    calls.lock().unwrap().clone()
}

fn steer(name: &str, text: Option<&str>, menu: Option<u32>) -> Request {
    Request::Steer(SteerReq {
        task: task(name),
        text: text.map(Into::into),
        menu,
    })
}

#[test]
fn repos_are_listed() {
    let (web, ..) = server_with_calls(vec![]);
    let resp = super::fetch(&web, "/repos", &[&bearer()]);
    assert!(resp.starts_with("HTTP/1.1 200"));
    assert_eq!(body(&resp), json!({"repos": ["app", "web"]}));
}

#[test]
fn start_sends_the_cli_request() {
    let (web, _, calls) = server_with_calls(vec![]);
    let resp = act(
        &web,
        "/start",
        &[],
        &json!({"repo": "app", "task": "fix-bug", "goal": "make it work"}),
    );
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    assert!(resp.contains("application/json"));
    let envelope = body(&resp);
    assert_eq!(envelope["class"], "ok");
    assert!(envelope["error"].is_null());
    let expected = Request::Start(StartReq {
        name: task("fix-bug"),
        goal: Some("make it work".into()),
        issue: None,
        worktree: None,
        base: None,
        model: None,
        no_agent: false,
        hold: false,
        resume: false,
        trust_moor_yaml: false,
        repo: Some("app".into()),
    });
    assert_eq!(recorded(&calls), vec![expected]);
}

#[test]
fn start_with_empty_goal_sends_none() {
    let (web, _, calls) = server_with_calls(vec![]);
    let resp = act(
        &web,
        "/start",
        &[],
        &json!({"repo": "app", "task": "t", "goal": ""}),
    );
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let calls = recorded(&calls);
    assert!(matches!(&calls[..], [Request::Start(r)] if r.goal.is_none()));
}

#[test]
fn steer_sends_text() {
    let (web, _, calls) = server_with_calls(vec![]);
    let resp = act(&web, "/steer", &[], &json!({"task": "t", "text": "hello"}));
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    assert_eq!(recorded(&calls), vec![steer("t", Some("hello"), None)]);
}

#[test]
fn answer_by_option_sends_the_menu_number() {
    let (web, _, calls) = server_with_calls(vec![]);
    let resp = act(&web, "/answer", &[], &json!({"task": "t", "option": 2}));
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    assert_eq!(recorded(&calls), vec![steer("t", None, Some(2))]);
}

#[test]
fn answer_by_text_sends_text() {
    let (web, _, calls) = server_with_calls(vec![]);
    let resp = act(&web, "/answer", &[], &json!({"task": "t", "text": "free"}));
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    assert_eq!(recorded(&calls), vec![steer("t", Some("free"), None)]);
}

#[test]
fn close_sends_a_plain_close() {
    let (web, _, calls) = server_with_calls(vec![]);
    let resp = act(&web, "/close", &[], &json!({"task": "t"}));
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let expected = Request::Close(CloseReq {
        task: task("t"),
        force: false,
        trust_moor_yaml: false,
    });
    assert_eq!(recorded(&calls), vec![expected]);
}

#[test]
fn refusal_envelope_passes_through_unchanged() {
    let (web, _, calls) = server_with_calls(vec![]);
    let resp = act(
        &web,
        "/start",
        &[],
        &json!({"repo": "nope", "task": "refused", "goal": "g"}),
    );
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let envelope = body(&resp);
    assert_eq!(envelope["class"], "refused");
    assert_eq!(envelope["error"]["kind"], "refused");
    assert_eq!(envelope["error"]["message"], REFUSAL);
    assert_eq!(envelope["error"]["hints"], json!(["moor repos"]));
    assert_eq!(recorded(&calls).len(), 1);
}

/// Asserts a 403 whose request never reached the feed.
fn assert_forbidden(web: &Web, calls: &Calls, headers: &[&str]) {
    let resp = post(web, "/close", headers, r#"{"task":"t"}"#);
    assert!(resp.starts_with("HTTP/1.1 403"), "{headers:?}: {resp}");
    assert!(resp.contains("forbidden"));
    assert!(recorded(calls).is_empty(), "{headers:?}");
}

/// POSTs with no `Host` header at all and returns the response.
fn post_without_host(web: &Web, headers: &[&str]) -> String {
    let body = r#"{"task":"t"}"#;
    let mut s = TcpStream::connect(web.addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        s,
        "POST /close HTTP/1.1\r\nContent-Length: {}\r\n",
        body.len()
    )
    .unwrap();
    for h in headers {
        write!(s, "{h}\r\n").unwrap();
    }
    write!(s, "\r\n{body}").unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out
}

#[test]
fn csrf_requirements_are_enforced() {
    let (web, _, calls) = server_with_calls(vec![]);
    let auth = bearer();
    let port = web.addr().port();
    let loopback = format!("Host: 127.0.0.1:{port}");
    let rejected: [&[&str]; 9] = [
        &[&auth, JSON],
        &[&auth, JSON, CSRF, "Origin: http://evil.example"],
        &[&auth, JSON, CSRF, "Origin: null"],
        &[&auth, JSON, CSRF, "Sec-Fetch-Site: cross-site"],
        &[&auth, "Content-Type: text/plain", CSRF],
        // Origin must match Host exactly, even between loopback names.
        &[
            &auth,
            JSON,
            CSRF,
            &loopback,
            &format!("Origin: http://localhost:{port}"),
        ],
        // A rebound name is rejected even when Origin matches it.
        &[
            &auth,
            JSON,
            CSRF,
            "Host: evil.example:80",
            "Origin: http://evil.example:80",
        ],
        &[&auth, JSON, CSRF, "Host: 127.0.0.1.evil.example"],
        &[&auth, JSON, CSRF, "Host: localhost:9999", "Origin: null"],
    ];
    for headers in rejected {
        assert_forbidden(&web, &calls, headers);
    }
    let resp = post_without_host(&web, &[&auth, JSON, CSRF]);
    assert!(resp.starts_with("HTTP/1.1 403"), "no host: {resp}");
    assert!(recorded(&calls).is_empty());
}

/// The browser's port may differ from the bound one (ssh -L).
#[test]
fn loopback_hosts_with_matching_origin_are_accepted() {
    let (web, _, calls) = server_with_calls(vec![]);
    let auth = bearer();
    let port = web.addr().port();
    let loopback = format!("Host: 127.0.0.1:{port}");
    let accepted: [&[&str]; 6] = [
        &[
            &auth,
            JSON,
            CSRF,
            "Host: localhost:9999",
            "Origin: http://localhost:9999",
        ],
        &[
            &auth,
            JSON,
            CSRF,
            "Host: [::1]:9999",
            "Origin: http://[::1]:9999",
        ],
        &[
            &auth,
            JSON,
            CSRF,
            "Host: 127.0.0.1",
            "Origin: http://127.0.0.1",
        ],
        &[&auth, JSON, CSRF, &loopback],
        &[
            &auth,
            JSON,
            CSRF,
            &loopback,
            &format!("Origin: http://127.0.0.1:{port}"),
        ],
        &[&auth, JSON, CSRF, &loopback, "Sec-Fetch-Site: same-origin"],
    ];
    for headers in accepted {
        let resp = post(&web, "/close", headers, r#"{"task":"t"}"#);
        assert!(resp.starts_with("HTTP/1.1 200"), "{headers:?}: {resp}");
    }
    assert_eq!(recorded(&calls).len(), accepted.len());
}

#[test]
fn post_needs_a_header_credential() {
    let (web, _, calls) = server_with_calls(vec![]);
    let b = r#"{"task":"t"}"#;
    let anon = post(&web, "/close", &[JSON, CSRF], b);
    assert!(anon.starts_with("HTTP/1.1 401"), "{anon}");
    let query = post(&web, &format!("/close?token={TOKEN}"), &[JSON, CSRF], b);
    assert!(query.starts_with("HTTP/1.1 401"), "{query}");
    assert!(recorded(&calls).is_empty());
}

#[test]
fn malformed_bodies_are_bad_requests() {
    let (web, _, calls) = server_with_calls(vec![]);
    let bad = [
        ("/close", "{not json".to_string()),
        ("/close", json!({"task": "t", "extra": 1}).to_string()),
        ("/close", json!({"task": "Not A Name"}).to_string()),
        (
            "/answer",
            json!({"task": "t", "option": 1, "text": "x"}).to_string(),
        ),
        ("/answer", json!({"task": "t"}).to_string()),
    ];
    let auth = bearer();
    for (path, b) in &bad {
        let resp = post(&web, path, &[&auth, JSON, CSRF], b);
        assert!(resp.starts_with("HTTP/1.1 400"), "{path} {b}: {resp}");
    }
    assert!(recorded(&calls).is_empty());
}

#[test]
fn oversize_body_is_rejected() {
    let (web, _, calls) = server_with_calls(vec![]);
    let len = format!("Content-Length: {}", 64 * 1024 + 1);
    let resp = post(&web, "/steer", &[&bearer(), JSON, CSRF, &len], "");
    assert!(resp.starts_with("HTTP/1.1 413"), "{resp}");
    assert!(recorded(&calls).is_empty());
}

#[test]
fn real_feed_refuses_an_unknown_repo_with_the_moor_text() {
    let dir = tempfile::TempDir::new().unwrap();
    let env = crate::env::Env::from_pairs([
        ("MOOR_STATE_DIR", dir.path().to_str().unwrap()),
        ("MOOR_CONFIG", "/nonexistent/moor.yaml"),
    ]);
    let factory = crate::controller::web_feed::factory(&env);
    let web = super::start(
        0,
        TOKEN.into(),
        factory,
        std::time::Duration::from_millis(10),
    )
    .unwrap();
    let resp = act(
        &web,
        "/start",
        &[],
        &json!({"repo": "nope", "task": "t", "goal": "g"}),
    );
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let message = body(&resp)["error"]["message"].as_str().unwrap().to_owned();
    assert!(
        message.contains("is not known to the controller"),
        "{message}"
    );
}
