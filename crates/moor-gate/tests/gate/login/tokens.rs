//! `/tokens` against a fake controller that keeps one person's API tokens in memory.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixListener;
use std::sync::{Arc, Mutex};

use moor_core::response::{ErrorKind, ErrorView, Response};
use moor_gate::{Gate, Login, Verifier};
use moor_testkit::oidc::Issuer;
use serde_json::{json, Value};

use super::{assert_page_headers, csrf, get, send, sign_in, Reply, CLIENT_ID};

/// When every fake token was made: 2026-11-01 10:13:27 UTC.
const CREATED_AT: i64 = 1_793_528_007_000;

type Store = Arc<Mutex<Vec<(i64, String, Value)>>>;

fn refused(message: &str) -> Response {
    let error = ErrorView {
        kind: ErrorKind::Usage,
        message: message.to_owned(),
        hints: vec![],
        context: vec![],
        detail: None,
    };
    Response::err("api-token-create", error)
}

fn answer(tokens: &mut Vec<(i64, String, Value)>, request: &Value) -> Response {
    let value = match request["cmd"].as_str().unwrap_or_default() {
        "api-token-create" => {
            let name = request["name"].as_str().unwrap().to_owned();
            if tokens.iter().any(|(_, n, _)| *n == name) {
                return refused(&format!("you already have a token named {name}"));
            }
            let id = i64::try_from(tokens.len()).unwrap() + 1;
            tokens.push((id, name, request["scopes"].clone()));
            json!({"id": id, "token": format!("moor_{:0>43}", format!("secret{id}wxyz"))})
        }
        "api-token-list" => {
            let listed: Vec<Value> = tokens
                .iter()
                .map(|(id, name, scopes)| {
                    json!({"id": id, "name": name, "scopes": scopes, "hint": "wxyz",
                        "created_at": CREATED_AT, "last_used_at": null})
                })
                .collect();
            json!({"tokens": listed})
        }
        "api-token-revoke" => {
            let id = request["id"].as_i64().unwrap();
            tokens.retain(|(i, _, _)| *i != id);
            json!({"id": id})
        }
        _ => Value::Null,
    };
    Response::ok("x", Some(value))
}

fn serve_controller(path: &std::path::Path, store: Store) {
    let listener = UnixListener::bind(path).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reply = stream.try_clone().unwrap();
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                let frame: Value = serde_json::from_str(&line).unwrap();
                if frame["frame"] == "hello" {
                    writeln!(reply, "{line}").unwrap();
                    continue;
                }
                assert_eq!(frame["request"]["identity"], "authgear:alice-sub");
                let envelope = answer(&mut store.lock().unwrap(), &frame["request"]["request"]);
                let out = json!({"frame": "response", "id": frame["id"], "envelope": envelope});
                writeln!(reply, "{out}").unwrap();
            }
        }
    });
}

/// A gate with sign-in on whose controller keeps tokens in the returned store.
fn gate() -> (String, Store, Issuer, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("authgear.sock");
    let store = Store::default();
    serve_controller(&socket, store.clone());
    let issuer = Issuer::start(CLIENT_ID);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let login = Login::new(&format!("http://{addr}")).unwrap();
    let gate = Arc::new(Gate {
        front_socket: socket,
        mcp: None,
        verifier: Verifier::new(issuer.base.clone(), CLIENT_ID.to_owned(), None),
    });
    std::thread::spawn(move || moor_gate::serve_with(&listener, &gate, Some(&Arc::new(login))));
    (addr, store, issuer, dir)
}

fn post(addr: &str, target: &str, cookie: &str, body: &str) -> Reply {
    let extra = format!("Cookie: {cookie}\r\nContent-Type: application/x-www-form-urlencoded\r\n");
    send(addr, "POST", target, &extra, body)
}

#[test]
fn tokens_sends_a_stranger_to_sign_in() {
    let (addr, _store, _issuer, _dir) = gate();
    let page = get(&addr, "/tokens", None);
    assert_eq!(page.status, 302);
    assert_eq!(page.location(), "/auth/login?return=/tokens");
}

fn assert_listed(body: &str, secret: &str) {
    assert!(
        body.contains("<td>grok-bot</td><td>moor:read</td>"),
        "{body}"
    );
    assert!(body.contains("\u{2026}wxyz"), "{body}");
    assert!(body.contains("2026-11-01 10:13 UTC"), "{body}");
    assert!(body.contains("never"), "{body}");
    assert!(!body.contains(secret), "the list never shows a token");
}

#[test]
fn a_token_is_created_shown_once_listed_by_hint_and_revoked() {
    let (addr, _store, _issuer, _dir) = gate();
    let cookie = sign_in(&addr);
    let empty = get(&addr, "/tokens", Some(&cookie));
    assert_eq!(empty.status, 200, "{}", empty.body);
    assert_page_headers(&empty);
    assert!(empty.body.contains("No tokens yet."), "{}", empty.body);
    let token = csrf(&empty);

    let created = post(
        &addr,
        "/tokens",
        &cookie,
        &format!("csrf={token}&name=grok-bot&read=on"),
    );
    assert_eq!(created.status, 200, "{}", created.body);
    assert_page_headers(&created);
    let secret = format!("moor_{:0>43}", "secret1wxyz");
    assert!(created.body.contains(&secret), "{}", created.body);
    assert!(created
        .body
        .contains("Copy it now; it won't be shown again."));
    assert!(created.body.contains("href=\"/tokens\""));

    assert_listed(&get(&addr, "/tokens", Some(&cookie)).body, &secret);

    let revoked = post(&addr, "/tokens/1/revoke", &cookie, &format!("csrf={token}"));
    assert_eq!(revoked.status, 303);
    assert_eq!(revoked.location(), "/tokens");
    let gone = get(&addr, "/tokens", Some(&cookie));
    assert!(gone.body.contains("No tokens yet."), "{}", gone.body);
}

#[test]
fn a_refused_create_shows_why_on_the_list_escaped() {
    let (addr, _store, _issuer, _dir) = gate();
    let cookie = sign_in(&addr);
    let token = csrf(&get(&addr, "/tokens", Some(&cookie)));
    let body = format!("csrf={token}&name=%3Cb%3Ebot&write=on");
    assert_eq!(post(&addr, "/tokens", &cookie, &body).status, 200);
    let again = post(&addr, "/tokens", &cookie, &body);
    assert_eq!(again.status, 200);
    assert!(
        again
            .body
            .contains("you already have a token named &lt;b&gt;bot"),
        "{}",
        again.body
    );
    assert!(again
        .body
        .contains("<td>&lt;b&gt;bot</td><td>moor:write</td>"));
    assert!(!again.body.contains("<b>bot"));
}

#[test]
fn posts_without_the_session_csrf_token_are_forbidden() {
    let (addr, store, _issuer, _dir) = gate();
    let cookie = sign_in(&addr);
    let token = csrf(&get(&addr, "/tokens", Some(&cookie)));
    post(
        &addr,
        "/tokens",
        &cookie,
        &format!("csrf={token}&name=kept&read=on"),
    );
    for (target, body) in [
        ("/tokens", "name=bot&read=on".to_owned()),
        ("/tokens", "csrf=wrong&name=bot&read=on".to_owned()),
        ("/tokens/1/revoke", String::new()),
        ("/tokens/1/revoke", "csrf=wrong".to_owned()),
    ] {
        let reply = post(&addr, target, &cookie, &body);
        assert_eq!(reply.status, 403, "{target} {body}");
    }
    let stranger = send(
        &addr,
        "POST",
        "/tokens/1/revoke",
        "",
        &format!("csrf={token}"),
    );
    assert_eq!(stranger.status, 403);
    assert_eq!(
        store.lock().unwrap().len(),
        1,
        "nothing was created or revoked"
    );
}

#[test]
fn tokens_is_not_found_without_a_public_url() {
    let w = super::super::world();
    assert!(super::super::http_get(&w, "/tokens").starts_with("HTTP/1.1 404"));
}
