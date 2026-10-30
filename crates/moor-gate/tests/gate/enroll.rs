//! `POST /machines/enroll` against the in-process issuer and a controller that answers every
//! `machine-enroll` with the request it got.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;

use moor_core::response::Response;
use serde_json::{json, Value};

use super::{authgear_token, mpsc, now, sign, world_with, World, CLIENT_ID};

const KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

fn serve_controller(path: &std::path::Path) -> mpsc::Receiver<String> {
    let listener = UnixListener::bind(path).unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reply = stream.try_clone().unwrap();
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                let frame: Value = serde_json::from_str(&line).unwrap();
                if frame["frame"] == "hello" {
                    writeln!(reply, "{line}").unwrap();
                    continue;
                }
                tx.send(line).unwrap();
                let envelope = Response::ok("machine-enroll", Some(frame["request"].clone()));
                let out = json!({"frame": "response", "id": frame["id"], "envelope": envelope});
                writeln!(reply, "{out}").unwrap();
            }
        }
    });
    rx
}

fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("authgear.sock");
    let seen = serve_controller(&socket);
    world_with(socket, seen, dir, false)
}

fn token(w: &World, change: &Value) -> String {
    let mut claims = json!({"iss": w.issuer, "sub": "alice-sub", "aud": CLIENT_ID,
        "exp": now() + 300, "email": "alice@example.com", "email_verified": true});
    for (k, v) in change.as_object().unwrap() {
        if v.is_null() {
            claims.as_object_mut().unwrap().remove(k);
        } else {
            claims[k] = v.clone();
        }
    }
    sign(&w.key, &claims)
}

fn post(w: &World, body: &Value) -> (u16, Value) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let mut res = agent
        .post(format!("http://{}/machines/enroll", w.gate))
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .unwrap();
    let status = res.status().as_u16();
    let text = res.body_mut().read_to_string().unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

fn enrollment(token: &str) -> Value {
    json!({"access_token": token, "pubkey": KEY, "name": "minion", "labels": ["macos"], "slots": 2})
}

#[test]
fn an_enrollment_reaches_the_controller_as_the_tokens_person_and_email() {
    let w = world();
    let (status, body) = post(&w, &enrollment(&token(&w, &json!({}))));
    assert_eq!(status, 200, "{body}");
    let sent: Value = serde_json::from_str(&w.seen.try_recv().unwrap()).unwrap();
    assert_eq!(
        sent["request"],
        json!({"cmd": "as", "identity": "authgear:alice-sub", "request": {
            "cmd": "machine-enroll", "email": "alice@example.com", "name": "minion",
            "pubkey": KEY, "labels": ["macos"], "capacity": 2}})
    );
    assert_eq!(body, sent["request"], "the controller's value comes back");
}

#[test]
fn a_token_for_another_client_expired_unverified_or_from_another_issuer_is_refused() {
    let w = world();
    for (change, why) in [
        (json!({"aud": "someone-else"}), "another audience"),
        (json!({"exp": now() - 600}), "ExpiredSignature"),
        (json!({"email_verified": false}), "not verified"),
        (
            json!({"iss": "https://issuer.example.com"}),
            "InvalidIssuer",
        ),
    ] {
        let (status, body) = post(&w, &enrollment(&token(&w, &change)));
        assert_eq!(status, 401, "{change}: {body}");
        let error = body["error"].as_str().unwrap();
        assert!(error.contains(why), "{change}: {error}");
    }
    assert!(w.seen.try_recv().is_err(), "nothing reached the controller");
}

#[test]
fn a_body_that_is_not_an_enrollment_is_refused_before_the_token_is_read() {
    let w = world();
    let (status, body) = post(&w, &json!({"access_token": "x", "name": "minion"}));
    assert_eq!(status, 400, "{body}");
    assert!(w.seen.try_recv().is_err());
}

#[test]
fn an_authgear_access_token_enrolls_with_the_userinfo_email() {
    let w = world();
    let (status, body) = post(&w, &enrollment(&authgear_token(&w)));
    assert_eq!(status, 200, "{body}");
    let sent: Value = serde_json::from_str(&w.seen.try_recv().unwrap()).unwrap();
    assert_eq!(sent["request"]["identity"], "authgear:alice-sub");
    assert_eq!(sent["request"]["request"]["email"], "alice@example.com");
}

#[test]
fn an_enrollment_still_naming_an_id_token_is_refused() {
    let w = world();
    let old = json!({"id_token": token(&w, &json!({})), "pubkey": KEY, "name": "minion"});
    let (status, body) = post(&w, &old);
    assert_eq!(status, 400, "{body}");
    assert!(w.seen.try_recv().is_err());
}
