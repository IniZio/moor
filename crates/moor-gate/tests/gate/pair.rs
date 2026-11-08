//! Pairing a machine by code: its agent asks at `/machines/pair`, a signed-in person confirms
//! at `/pair`, and the agent's poll gets the controller's enrollment once.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixListener;
use std::sync::{Arc, Mutex};

use moor_core::response::Response;
use moor_gate::{Gate, Login, Verifier};
use moor_testkit::oidc::Issuer;
use oauth2::url::form_urlencoded;
use serde_json::{json, Value};

use super::login::{csrf, get, send, sign_in, Reply};
use super::CLIENT_ID;

const KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

struct World {
    addr: String,
    /// Every `machine-enroll` the controller got, with the identity it came as.
    enrolled: Arc<Mutex<Vec<Value>>>,
    _issuer: Issuer,
    _dir: tempfile::TempDir,
}

/// A gate in front of a controller that enrolls every machine as id 7; with browser sign-in
/// when `login`.
fn world(login: bool) -> World {
    let issuer = Issuer::start(CLIENT_ID);
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("authgear.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let enrolled: Arc<Mutex<Vec<Value>>> = Arc::default();
    let kept = Arc::clone(&enrolled);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let kept = Arc::clone(&kept);
            std::thread::spawn(move || {
                let mut reply = stream.try_clone().unwrap();
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let frame: Value = serde_json::from_str(&line).unwrap();
                    if frame["frame"] == "hello" {
                        writeln!(reply, "{line}").unwrap();
                        continue;
                    }
                    let inner = &frame["request"]["request"];
                    let value = if inner["cmd"] == "machine-enroll" {
                        kept.lock().unwrap().push(frame["request"].clone());
                        json!({"machine_id": 7, "name": inner["name"], "user": 1,
                            "owner": "alice@example.com", "capacity": inner["capacity"]})
                    } else {
                        json!({"identity": frame["request"]["identity"]})
                    };
                    let envelope = Response::ok("x", Some(value));
                    let out = json!({"frame": "response", "id": frame["id"], "envelope": envelope});
                    writeln!(reply, "{out}").unwrap();
                }
            });
        }
    });
    let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = tcp.local_addr().unwrap().to_string();
    let login = login.then(|| Arc::new(Login::new(&format!("http://{addr}")).unwrap()));
    let gate = Arc::new(Gate {
        controller: moor_client::Client::new(socket),
        mcp: None,
        verifier: Verifier::new(issuer.base.clone(), CLIENT_ID.to_owned(), None),
    });
    std::thread::spawn(move || moor_gate::serve_with(&tcp, &gate, login.as_ref()));
    World {
        addr,
        enrolled,
        _issuer: issuer,
        _dir: dir,
    }
}

impl World {
    /// Posts `body` as JSON to `path`: the status and the JSON answer.
    fn post_json(&self, path: &str, body: &Value) -> (u16, Value) {
        let extra = "Content-Type: application/json\r\n";
        let reply = send(&self.addr, "POST", path, extra, &body.to_string());
        (reply.status, serde_json::from_str(&reply.body).unwrap())
    }

    /// Starts pairing machine `hv`: the gate's answer.
    fn ask(&self) -> Value {
        let (status, answer) = self.post_json(
            "/machines/pair",
            &json!({"pubkey": KEY, "name": "hv", "slots": 4, "pool": "cloudhypervisor"}),
        );
        assert_eq!(status, 200, "{answer}");
        answer
    }

    fn poll(&self, device_code: &Value) -> (u16, Value) {
        self.post_json("/machines/pair/poll", &json!({"device_code": device_code}))
    }

    /// Posts the confirm form for `code` with `session` and its `csrf`.
    fn confirm(&self, session: &str, csrf: &str, code: &str) -> Reply {
        let body = form_urlencoded::Serializer::new(String::new())
            .extend_pairs([("csrf", csrf), ("code", code)])
            .finish();
        let extra =
            format!("Content-Type: application/x-www-form-urlencoded\r\nCookie: {session}\r\n");
        send(&self.addr, "POST", "/pair", &extra, &body)
    }
}

/// The path and query of `url` on the gate.
fn target(url: &str) -> String {
    let rest = url.strip_prefix("http://").unwrap();
    format!("/{}", rest.split_once('/').unwrap().1)
}

/// Checks the code and URLs `asked` hands out, as RFC 8628 names them.
fn assert_handed_out(w: &World, asked: &Value) {
    let code = asked["user_code"].as_str().unwrap();
    assert!(
        code.len() == 9 && code.as_bytes()[4] == b'-',
        "a code reads XXXX-XXXX: {code}"
    );
    assert!(code.chars().all(|c| c == '-' || c.is_ascii_uppercase()));
    assert_eq!(asked["verification_uri"], format!("http://{}/pair", w.addr));
    assert_eq!(
        asked["verification_uri_complete"],
        format!("http://{}/pair?code={code}", w.addr)
    );
    assert_eq!(asked["expires_in"], 600);
    assert_eq!(asked["interval"], 5);
}

/// Checks the controller got one `machine-enroll` of `hv` as alice.
fn assert_enrolled_once_as_alice(w: &World) {
    let enrolled = w.enrolled.lock().unwrap().clone();
    assert_eq!(enrolled.len(), 1);
    assert_eq!(enrolled[0]["identity"], "authgear:alice-sub");
    let req = &enrolled[0]["request"];
    assert_eq!(req["name"], "hv");
    assert_eq!(req["pubkey"], KEY);
    assert_eq!(req["capacity"], 4);
    assert_eq!(req["pool"], "cloudhypervisor");
}

/// Opens `url` as a browser would: sent to sign in first, then shown the page. The session
/// and the page.
fn open_signed_in(w: &World, url: &str) -> (String, Reply) {
    let page = target(url);
    let anonymous = get(&w.addr, &page, None);
    assert_eq!(anonymous.status, 302);
    assert!(
        anonymous.location().starts_with("/auth/login?return="),
        "{}",
        anonymous.location()
    );
    let session = sign_in(&w.addr);
    let shown = get(&w.addr, &page, Some(&session));
    assert_eq!(shown.status, 200, "{}", shown.body);
    (session, shown)
}

#[test]
fn a_confirmed_code_enrolls_the_machine_as_its_confirmer_and_answers_one_poll() {
    let w = world(true);
    let asked = w.ask();
    assert_handed_out(&w, &asked);
    let code = asked["user_code"].as_str().unwrap();
    assert_eq!(
        w.poll(&asked["device_code"]),
        (400, json!({"error": "authorization_pending"}))
    );

    let (session, shown) = open_signed_in(&w, asked["verification_uri_complete"].as_str().unwrap());
    assert!(shown.body.contains("hv"), "{}", shown.body);
    assert!(shown.body.contains(code), "{}", shown.body);
    assert!(
        w.enrolled.lock().unwrap().is_empty(),
        "showing enrolls nothing"
    );

    let done = w.confirm(&session, &csrf(&shown), code);
    assert_eq!(done.status, 200, "{}", done.body);
    assert_enrolled_once_as_alice(&w);

    let enrolled = json!({"machine_id": 7, "name": "hv", "user": 1, "owner": "alice@example.com",
        "capacity": 4});
    assert_eq!(w.poll(&asked["device_code"]), (200, enrolled));
    assert_eq!(
        w.poll(&asked["device_code"]),
        (400, json!({"error": "expired_token"})),
        "the answer is handed out once"
    );
}

#[test]
fn a_used_code_is_refused_and_enrolls_nothing_more() {
    let w = world(true);
    let asked = w.ask();
    let code = asked["user_code"].as_str().unwrap();
    let session = sign_in(&w.addr);
    let shown = get(&w.addr, &format!("/pair?code={code}"), Some(&session));
    let token = csrf(&shown);
    assert_eq!(w.confirm(&session, &token, code).status, 200);

    let again = w.confirm(&session, &token, code);
    assert_eq!(again.status, 404, "{}", again.body);
    assert!(
        again.body.contains("moor-machine-agent up"),
        "{}",
        again.body
    );
    let shown = get(&w.addr, &format!("/pair?code={code}"), Some(&session));
    assert_eq!(shown.status, 404, "{}", shown.body);
    assert_eq!(w.enrolled.lock().unwrap().len(), 1);
}

#[test]
fn a_code_typed_in_lower_case_without_its_dash_still_pairs() {
    let w = world(true);
    let asked = w.ask();
    let typed = asked["user_code"]
        .as_str()
        .unwrap()
        .replace('-', "")
        .to_lowercase();
    let session = sign_in(&w.addr);
    let shown = get(&w.addr, &format!("/pair?code={typed}"), Some(&session));
    assert_eq!(shown.status, 200, "{}", shown.body);
    assert_eq!(w.confirm(&session, &csrf(&shown), &typed).status, 200);
    assert_eq!(w.poll(&asked["device_code"]).0, 200);
}

#[test]
fn a_confirm_without_the_sessions_csrf_token_enrolls_nothing() {
    let w = world(true);
    let asked = w.ask();
    let code = asked["user_code"].as_str().unwrap();
    let session = sign_in(&w.addr);
    let refused = w.confirm(&session, "forged", code);
    assert_eq!(refused.status, 403, "{}", refused.body);
    assert!(w.enrolled.lock().unwrap().is_empty());
    assert_eq!(
        w.poll(&asked["device_code"]),
        (400, json!({"error": "authorization_pending"}))
    );
}

#[test]
fn an_unknown_device_code_reads_as_expired() {
    let w = world(true);
    assert_eq!(
        w.poll(&json!("nope")),
        (400, json!({"error": "expired_token"}))
    );
}

#[test]
fn a_gate_without_browser_sign_in_says_it_cannot_pair() {
    let w = world(false);
    let (status, answer) = w.post_json(
        "/machines/pair",
        &json!({"pubkey": KEY, "name": "hv", "slots": 1}),
    );
    assert_eq!(status, 501, "{answer}");
    assert!(
        answer["error"].as_str().unwrap().contains("--public-url"),
        "{answer}"
    );
}

#[test]
fn a_pairing_that_is_not_json_is_a_bad_request() {
    let w = world(true);
    let reply = send(&w.addr, "POST", "/machines/pair", "", "{");
    assert_eq!(reply.status, 400, "{}", reply.body);
}
