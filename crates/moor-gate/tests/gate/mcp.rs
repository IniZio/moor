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
                    let box_name = inner["name"].as_str().or_else(|| inner["box"].as_str());
                    let cmd = inner["cmd"].as_str().unwrap_or_default();
                    if cmd == "start" {
                        started.lock().unwrap().insert(box_name.unwrap().to_owned());
                        if box_name.unwrap().ends_with("/slow") {
                            std::thread::sleep(std::time::Duration::from_secs(3));
                        }
                    }
                    if cmd == "show"
                        && inner["box"].is_string()
                        && !is_known(&started, box_name.unwrap())
                    {
                        writeln!(reply, "{}", not_found(&frame["id"])).unwrap();
                        continue;
                    }
                    let value = match cmd {
                        "show" if inner["box"].is_string() => shown(&inner["box"]),
                        "show" => {
                            json!({"boxes": [{"name": format!("repo/{}", who.replace(':', "-")),
                                "state": "ready"},
                                {"name": "repo/failed", "lifecycle": "setup_failed"},
                                {"name": "repo/starting", "lifecycle": "setting_up"}]})
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

/// The controller's answer to frame `id` for a `show` of a Box that does not exist.
fn not_found(id: &Value) -> Value {
    let error = ErrorView {
        kind: ErrorKind::NotFound,
        message: "no such box".to_owned(),
        hints: vec![],
        context: vec![],
        detail: None,
    };
    json!({"frame": "response", "id": id, "envelope": Response::err("show", error)})
}

/// Whether `show` of the Box `name` finds it: one of the fixed Boxes or one started.
fn is_known(started: &Mutex<HashSet<String>>, name: &str) -> bool {
    matches!(name, "repo/failed" | "repo/starting") || started.lock().unwrap().contains(name)
}

const ACQUIRE_FAILED: &str = "box acquire failed on provider external (exit 1): \
    HTTP 502 provisioning_failed; releasing the lease it started also failed";

/// One Box with its events, as `show` of a named Box answers.
fn shown(name: &Value) -> Value {
    let event =
        |kind: &str, payload: Value, ts: &str| json!({"kind": kind, "payload": payload, "ts": ts});
    let (lifecycle, events) = match name.as_str().unwrap_or_default() {
        "repo/failed" => (
            "setup_failed",
            vec![
                event("started", json!({}), "2026-10-29T10:00:00Z"),
                event("box_requested", json!({}), "2026-10-29T10:00:01Z"),
                event(
                    "box_failed",
                    json!({"error": ACQUIRE_FAILED}),
                    "2026-10-29T10:01:00Z",
                ),
            ],
        ),
        _ => (
            "setting_up",
            vec![
                event("started", json!({}), "2026-10-29T10:00:00Z"),
                event("box_requested", json!({}), "2026-10-29T10:00:01Z"),
                event(
                    "credential_seen",
                    json!({"token": "s3cret"}),
                    "2026-10-29T10:00:02Z",
                ),
                event(
                    "box_progress",
                    json!({"text": "waiting for SSH (1m6s)"}),
                    "2026-10-29T10:01:07Z",
                ),
            ],
        ),
    };
    json!({"box": {"name": name, "lifecycle": lifecycle}, "events": events})
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
    assert_eq!(
        result["structuredContent"]["box"]["name"], "web/slow",
        "{result}"
    );
    let next = result["structuredContent"]["next"].as_str().unwrap();
    assert!(next.contains("box_status"), "{result}");
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
    let next = result["structuredContent"]["next"].as_str().unwrap();
    assert!(next.contains("box_status"), "{result}");
}

#[test]
fn list_boxes_says_why_a_box_failed_and_what_a_starting_one_waits_on() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let reply = m.call(&token, "list_boxes", json!({}));
    let boxes = &reply["structuredContent"]["boxes"];
    assert_eq!(
        boxes[1]["status"],
        format!("machine failed: {ACQUIRE_FAILED}")
    );
    let starting = boxes[2]["status"].as_str().unwrap();
    assert!(starting.starts_with("starting · "), "{starting}");
    assert!(
        starting.ends_with(" · waiting for SSH (1m6s)"),
        "{starting}"
    );
}

#[test]
fn box_status_carries_the_failure_text() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let reply = m.call(&token, "box_status", json!({"box": "repo/failed"}));
    let shown = &reply["structuredContent"];
    assert_eq!(
        shown["failure"],
        format!("machine failed: {ACQUIRE_FAILED}")
    );
    assert_eq!(shown["status"], format!("machine failed: {ACQUIRE_FAILED}"));
}

#[test]
fn box_status_asks_for_the_agent_artifacts_and_reports() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    m.call(&token, "box_status", json!({"box": "repo/failed"}));
    std::thread::sleep(std::time::Duration::from_millis(100));
    let asked: Vec<Value> = m
        .seen
        .try_iter()
        .map(|l| serde_json::from_str(&l).unwrap())
        .collect();
    let inner = &asked.last().unwrap()["request"]["request"];
    assert_eq!(
        (&inner["agent"], &inner["artifacts"], &inner["reports"]),
        (&json!(true), &json!(true), &json!(true))
    );
}

#[test]
fn box_status_carries_a_starting_box_s_steps_and_nothing_the_card_hides() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let reply = m.call(&token, "box_status", json!({"box": "repo/starting"}));
    let steps = reply["structuredContent"]["steps"].as_array().unwrap();
    let steps: Vec<&str> = steps
        .iter()
        .map(|s| s.as_str().unwrap().trim_start())
        .collect();
    assert_eq!(
        steps,
        ["+1s  machine requested", "+1m 07s  waiting for SSH (1m6s)"]
    );
    assert!(
        reply["structuredContent"].get("failure").is_none(),
        "{reply}"
    );
}

#[test]
fn start_box_tells_the_client_to_follow_it_with_box_status() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read moor:write");
    let result = m.call(
        &token,
        "start_box",
        json!({"repo": "acme/web", "name": "web/fix"}),
    );
    let next = result["structuredContent"]["next"].as_str().unwrap();
    assert!(next.contains("box_status"), "{result}");
    let reply = m.rpc(&token, "tools/list", json!({}));
    let tools = reply["result"]["tools"].as_array().unwrap();
    let described = |name: &str| {
        tools.iter().find(|t| t["name"] == name).unwrap()["description"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert!(described("start_box").contains("box_status"));
    assert!(described("list_boxes").contains("why it failed or what it is waiting on"));
}

#[test]
fn start_box_passes_the_node_to_the_start() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read moor:write");
    let result = m.call(
        &token,
        "start_box",
        json!({"repo": "acme/web", "name": "web/fix", "prompt": "fix it", "node": "hv"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    let sent = m.requests();
    assert_eq!(sent[0]["request"]["request"]["node"], "hv", "{sent:?}");
}

#[test]
fn close_box_refuses_unsaved_work_unless_force_is_set() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read moor:write");
    for (args, force) in [
        (json!({"box": "web/fix"}), false),
        (json!({"box": "web/fix", "force": false}), false),
        (json!({"box": "web/fix", "force": true}), true),
    ] {
        let result = m.call(&token, "close_box", args);
        assert_ne!(result["isError"], true, "{result}");
        let sent = m.requests();
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert_eq!(
            sent[0]["request"],
            json!({"cmd": "as", "identity": "authgear:alice-sub", "request": {
                "cmd": "close", "box": "web/fix", "force": force}})
        );
    }
}

#[test]
fn close_box_lists_force_as_an_optional_boolean() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let reply = m.rpc(&token, "tools/list", json!({}));
    let close = reply["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "close_box")
        .unwrap();
    let schema = &close["inputSchema"];
    assert_eq!(schema["required"], json!(["box"]), "{schema}");
    let force = &schema["properties"]["force"];
    assert!(
        force["type"] == "boolean" || force["type"] == json!(["boolean", "null"]),
        "{schema}"
    );
    assert!(
        close["description"].as_str().unwrap().contains("force"),
        "{close}"
    );
}
