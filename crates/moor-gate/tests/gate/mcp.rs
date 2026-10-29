//! `/mcp` against the in-process issuer and a fake controller that answers like the real one:
//! every request arrives wrapped in `as`, and `show` answers only for the identity it names.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::sync::{Arc, Mutex};

use moor_core::response::{ErrorKind, ErrorView, Response};
use serde_json::{json, Value};

use std::sync::atomic::Ordering;

use super::{claims, mpsc, now, sign, world_with, World};

fn serve_controller(path: &std::path::Path) -> mpsc::Receiver<String> {
    let listener = UnixListener::bind(path).unwrap();
    let (tx, rx) = mpsc::channel();
    let started = Arc::new(Mutex::new(HashSet::new()));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            let started = started.clone();
            std::thread::spawn(move || {
                let mut reply = stream.try_clone().unwrap();
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let frame: Value = serde_json::from_str(&line).unwrap();
                    if frame["frame"] == "hello" {
                        writeln!(reply, "{line}").unwrap();
                        continue;
                    }
                    tx.send(line).unwrap();
                    let inner = &frame["request"]["request"];
                    let who = frame["request"]["identity"].as_str().unwrap_or_default();
                    let box_name = inner["name"].as_str().or(inner["box"].as_str());
                    let cmd = inner["cmd"].as_str().unwrap_or_default();
                    if cmd == "start" {
                        started.lock().unwrap().insert(box_name.unwrap().to_owned());
                        if box_name.unwrap().ends_with("/slow") {
                            std::thread::sleep(std::time::Duration::from_secs(3));
                        }
                    }
                    if cmd == "show"
                        && inner["box"].is_string()
                        && !started.lock().unwrap().contains(box_name.unwrap())
                    {
                        let error = ErrorView {
                            kind: ErrorKind::NotFound,
                            message: "no such box".to_owned(),
                            hints: vec![],
                            context: vec![],
                            detail: None,
                        };
                        let out = json!({"frame": "response", "id": frame["id"],
                            "envelope": Response::err("show", error)});
                        writeln!(reply, "{out}").unwrap();
                        continue;
                    }
                    let value = match cmd {
                        "show" => {
                            json!({"boxes": [{"name": format!("repo/{}", who.replace(':', "-")),
                                "state": "ready"}]})
                        }
                        "start" => json!({"started": inner["name"]}),
                        _ => Value::Null,
                    };
                    let envelope = Response::ok("x", Some(value));
                    let out = json!({"frame": "response", "id": frame["id"], "envelope": envelope});
                    writeln!(reply, "{out}").unwrap();
                }
            });
        }
    });
    rx
}

/// A `show` of one Box by name, which `start_box` sends to see whether the Box exists yet.
fn is_existence_probe(frame: &Value) -> bool {
    frame["request"]["request"]["cmd"] == "show" && frame["request"]["request"]["box"].is_string()
}

struct Mcp {
    world: World,
    seen: mpsc::Receiver<String>,
}

fn mcp() -> Mcp {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("authgear.sock");
    let seen = serve_controller(&socket);
    let (_unused, rx) = mpsc::channel();
    Mcp {
        world: world_with(socket, rx, dir, true),
        seen,
    }
}

impl Mcp {
    fn url(&self) -> String {
        format!("http://{}/mcp", self.world.gate)
    }

    fn token(&self, sub: &str, aud: &str, scope: &str) -> String {
        let mut c = claims(&self.world);
        c["aud"] = json!(aud);
        c["sub"] = json!(sub);
        c["scope"] = json!(scope);
        c["exp"] = json!(now() + 600);
        sign(&self.world.key, &c)
    }

    fn post(
        &self,
        token: Option<&str>,
        body: &Value,
        session: Option<&str>,
    ) -> (u16, Value, String) {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into();
        let mut req = agent
            .post(self.url())
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");
        if let Some(token) = token {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        if let Some(session) = session {
            req = req.header("Mcp-Session-Id", session);
        }
        let mut res = req.send(body.to_string()).unwrap();
        let status = res.status().as_u16();
        let challenge = res
            .headers()
            .get("www-authenticate")
            .map(|v| v.to_str().unwrap().to_owned())
            .unwrap_or_default();
        let text = res.body_mut().read_to_string().unwrap();
        let json = serde_json::from_str(&text).unwrap_or(Value::Null);
        (status, json, challenge)
    }

    /// The JSON-RPC reply to `method` after a full handshake with `token`.
    fn rpc(&self, token: &str, method: &str, params: Value) -> Value {
        let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "test", "version": "0"}}});
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into();
        let send = |body: &Value, session: Option<&str>| {
            let mut req = agent
                .post(self.url())
                .header("Content-Type", "application/json")
                .header("Accept", "application/json, text/event-stream")
                .header("Authorization", format!("Bearer {token}"));
            if let Some(session) = session {
                req = req.header("Mcp-Session-Id", session);
            }
            let mut res = req.send(body.to_string()).unwrap();
            let session = res
                .headers()
                .get("mcp-session-id")
                .map(|v| v.to_str().unwrap().to_owned());
            let text = res.body_mut().read_to_string().unwrap();
            (
                serde_json::from_str::<Value>(&text).unwrap_or(Value::Null),
                session,
            )
        };
        let (opened, session) = send(&init, None);
        assert!(opened.get("result").is_some(), "initialize: {opened}");
        send(
            &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            session.as_deref(),
        );
        send(
            &json!({"jsonrpc": "2.0", "id": 2, "method": method, "params": params}),
            session.as_deref(),
        )
        .0
    }

    fn call(&self, token: &str, tool: &str, args: Value) -> Value {
        let reply = self.rpc(
            token,
            "tools/call",
            json!({"name": tool, "arguments": args}),
        );
        reply["result"].clone()
    }

    /// The request lines the controller got that are not `enroll`.
    fn requests(&self) -> Vec<Value> {
        std::thread::sleep(std::time::Duration::from_millis(100));
        self.seen
            .try_iter()
            .map(|l| serde_json::from_str::<Value>(&l).unwrap())
            .filter(|f| f["request"]["request"]["cmd"] != "enroll")
            .filter(|f| !is_existence_probe(f))
            .collect()
    }
}

#[test]
fn an_unauthenticated_call_gets_401_pointing_at_the_resource_metadata() {
    let m = mcp();
    let (status, _, challenge) = m.post(None, &json!({}), None);
    assert_eq!(status, 401);
    let want = format!(
        "Bearer resource_metadata=\"http://{}/.well-known/oauth-protected-resource\"",
        m.world.gate
    );
    assert_eq!(challenge, want);
    let doc = super::http_get(&m.world, "/.well-known/oauth-protected-resource");
    let doc: Value = serde_json::from_str(doc.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(doc["resource"], m.url());
    assert_eq!(doc["authorization_servers"], json!([m.world.issuer]));
    assert_eq!(
        doc["scopes_supported"],
        json!(["openid", "offline_access", "moor:read", "moor:write"])
    );
}

#[test]
fn a_token_for_another_audience_is_refused() {
    let m = mcp();
    let front = m.token("alice-sub", super::CLIENT_ID, "moor:read moor:write");
    let (status, _, _) = m.post(Some(&front), &json!({}), None);
    assert_eq!(status, 401);
}

#[test]
fn an_authgear_token_for_the_gate_s_client_is_refused() {
    let m = mcp();
    let front = super::authgear_token(&m.world);
    let (status, _, _) = m.post(Some(&front), &json!({}), None);
    assert_eq!(status, 401);
}

#[test]
fn a_resource_token_without_email_acts_with_the_userinfo_email() {
    let m = mcp();
    let mut c = claims(&m.world);
    c["aud"] = json!([m.url()]);
    c["client_id"] = json!("an-mcp-client");
    c["scope"] = json!("moor:read");
    c.as_object_mut().unwrap().remove("email");
    c.as_object_mut().unwrap().remove("email_verified");
    let token = sign(&m.world.key, &c);
    let reply = m.call(&token, "list_boxes", json!({}));
    assert_eq!(
        reply["structuredContent"]["boxes"][0]["name"],
        "repo/authgear-alice-sub"
    );
    m.call(&token, "list_boxes", json!({}));
    assert_eq!(m.world.userinfo_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn the_tools_are_listed() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let reply = m.rpc(&token, "tools/list", json!({}));
    let mut names: Vec<&str> = reply["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "box_status",
            "close_box",
            "list_boxes",
            "send_to_box",
            "start_box"
        ]
    );
}

#[test]
fn a_read_token_cannot_start_a_box() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let result = m.call(
        &token,
        "start_box",
        json!({"repo": "acme/web", "prompt": "hi"}),
    );
    assert_eq!(result["isError"], true, "{result}");
    assert!(
        result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("moor:write"),
        "{result}"
    );
    assert!(m.requests().is_empty());
}

#[test]
fn list_boxes_answers_for_the_token_s_person_only() {
    let m = mcp();
    let alice = m.token("alice-sub", &m.url(), "moor:read");
    let bob = m.token("bob-sub", &m.url(), "moor:read");
    let mine = m.call(&alice, "list_boxes", json!({}));
    let theirs = m.call(&bob, "list_boxes", json!({}));
    assert_eq!(
        mine["structuredContent"]["boxes"][0]["name"],
        "repo/authgear-alice-sub"
    );
    assert_eq!(
        theirs["structuredContent"]["boxes"][0]["name"],
        "repo/authgear-bob-sub"
    );
}

#[test]
fn start_box_sends_a_start_as_the_person() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read moor:write");
    let result = m.call(
        &token,
        "start_box",
        json!({"repo": "acme/web", "name": "web/fix", "prompt": "fix it", "base": "main"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    let sent = m.requests();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(
        sent[0]["request"],
        json!({"cmd": "as", "identity": "authgear:alice-sub", "request": {
            "cmd": "start", "name": "web/fix", "prompt": "fix it", "base": "main",
            "repo": "acme/web", "no_agent": false, "resume": false}})
    );
}

#[test]
fn start_box_returns_while_the_start_is_still_running() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read moor:write");
    let began = std::time::Instant::now();
    let result = m.call(
        &token,
        "start_box",
        json!({"repo": "acme/web", "name": "web/slow"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert!(began.elapsed() < std::time::Duration::from_secs(2));
    assert!(result["structuredContent"]["boxes"].is_array(), "{result}");
}

#[test]
fn start_box_with_wait_returns_the_start_result() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read moor:write");
    let began = std::time::Instant::now();
    let result = m.call(
        &token,
        "start_box",
        json!({"repo": "acme/web", "name": "web/slow", "wait": true}),
    );
    assert!(began.elapsed() >= std::time::Duration::from_secs(3));
    assert_eq!(result["structuredContent"]["started"], "web/slow");
}
