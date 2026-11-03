//! `/mcp` resources and `subscriptions/listen` on MCP 2026-07-28, against a fake controller
//! that serves watches: a backlog watch caught up at [`TIP`], and live watches that get each
//! event the test emits.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use moor_core::controller::ServerFrame;
use moor_core::response::Response;
use moor_core::BoxId;
use serde_json::{json, Value};

use super::{claims, now, sign, world_with, World};

/// The cursor a backlog watch is caught up to.
pub(super) const TIP: i64 = 10;

/// What the fake controller saw and holds.
#[derive(Default)]
pub(super) struct Fake {
    /// Every watch frame, in order.
    pub(super) watches: Vec<Value>,
    /// Each live watch.
    pub(super) live: Vec<UnixStream>,
    /// Live watches whose client hung up.
    ended: usize,
    /// The event log every watch replays: offset, Box name and kind.
    pub(super) log: Vec<(i64, String, String)>,
}

/// Serves `path`; the shared state records watches and holds the live ones.
pub(super) fn serve_controller(path: &std::path::Path) -> Arc<Mutex<Fake>> {
    let listener = UnixListener::bind(path).unwrap();
    let fake = Arc::new(Mutex::new(Fake::default()));
    let shared = Arc::clone(&fake);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let fake = Arc::clone(&shared);
            std::thread::spawn(move || answer(stream, &fake));
        }
    });
    fake
}

/// Answers one connection's frames.
fn answer(stream: UnixStream, fake: &Mutex<Fake>) {
    let mut reply = stream.try_clone().unwrap();
    let mut following = false;
    for line in BufReader::new(stream).lines().map_while(Result::ok) {
        let frame: Value = serde_json::from_str(&line).unwrap();
        match frame["frame"].as_str().unwrap() {
            "hello" => writeln!(reply, "{line}").unwrap(),
            "watch" => {
                // The wire leaves `follow` out when it is true.
                let follow = frame["follow"] != false;
                let follow = replay(fake, frame, &mut reply, follow);
                following |= follow;
                if !follow {
                    return;
                }
            }
            _ => {
                let inner = &frame["request"]["request"];
                let value = match (inner["cmd"].as_str(), inner["box"].as_str()) {
                    (Some("show"), Some(name)) => json!({
                        "box": {"name": name, "lifecycle": "ready",
                            "said": {"text": "Opened the PR.", "at": "2026-11-01T00:00:00Z"}},
                        "events": [{"kind": "idled", "payload": {}, "ts": "2026-11-01T00:00:00Z"}],
                    }),
                    (Some("show"), None) => json!({"boxes": [
                        {"name": "repo/a", "lifecycle": "ready"},
                        {"name": "repo/b", "lifecycle": "ready"},
                    ]}),
                    _ => json!({}),
                };
                let envelope = Response::ok("x", Some(value));
                let out = json!({"frame": "response", "id": frame["id"], "envelope": envelope});
                writeln!(reply, "{out}").unwrap();
            }
        }
    }
    if following {
        fake.lock().unwrap().ended += 1;
    }
}

/// Writes the log events a `watch` frame asks for, then caught-up; holds the connection for
/// live events when `follow`. Gives whether it is held.
fn replay(fake: &Mutex<Fake>, frame: Value, reply: &mut UnixStream, follow: bool) -> bool {
    let since = frame["since"].as_i64().unwrap_or(0);
    let kinds = frame["kinds"].as_array().cloned();
    let boxes = frame["boxes"].as_array().cloned();
    let mut fake = fake.lock().unwrap();
    fake.watches.push(frame);
    let mut cursor = TIP;
    for (id, name, kind) in fake.log.clone() {
        cursor = cursor.max(id);
        let listed = |list: &Option<Vec<Value>>, v: &str| {
            list.as_ref().is_none_or(|l| l.iter().any(|x| x == v))
        };
        if id > since && listed(&kinds, &kind) && listed(&boxes, &name) {
            let event = event_frame(id, &name, &kind);
            reply.write_all(event.to_line().as_bytes()).unwrap();
        }
    }
    let up = ServerFrame::CaughtUp { cursor };
    reply.write_all(up.to_line().as_bytes()).unwrap();
    if follow {
        fake.live.push(reply.try_clone().unwrap());
    }
    follow
}

/// The frame of event `cursor`, of `kind` for Box `name`.
fn event_frame(cursor: i64, name: &str, kind: &str) -> ServerFrame {
    ServerFrame::Event {
        cursor,
        r#box: Some(name.to_owned()),
        box_id: Some(BoxId::new(cursor)),
        session: None,
        kind: kind.to_owned(),
        payload: json!({}),
        ts: 0,
    }
}

/// Sends an event of `kind` for Box `name` to every live watch.
pub(super) fn emit(fake: &Mutex<Fake>, cursor: i64, name: &str, kind: &str) {
    let frame = event_frame(cursor, name, kind);
    for live in &mut fake.lock().unwrap().live {
        let _ = live.write_all(frame.to_line().as_bytes());
    }
}

/// A gate with `/mcp` over the fake controller.
pub(super) fn world() -> (World, Arc<Mutex<Fake>>) {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("authgear.sock");
    let fake = serve_controller(&socket);
    let (_unused, rx) = mpsc::channel();
    (world_with(socket, rx, dir, true), fake)
}

/// A token for the gate's `/mcp` with `scope`.
pub(super) fn token(world: &World, scope: &str) -> String {
    let mut c = claims(world);
    c["aud"] = json!(format!("http://{}/mcp", world.gate));
    c["scope"] = json!(scope);
    c["exp"] = json!(now() + 600);
    sign(&world.key, &c)
}

/// POSTs `method` with `params` as a 2026-07-28 request, and hands back each JSON-RPC
/// message of the answer as it arrives, from a JSON body or an SSE stream.
pub(super) fn post(
    world: &World,
    token: &str,
    method: &str,
    mut params: Value,
) -> mpsc::Receiver<Value> {
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "test", "version": "0"},
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let mut req = agent
        .post(format!("http://{}/mcp", world.gate))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Authorization", format!("Bearer {token}"))
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", method);
    if let Some(name) = params["uri"].as_str().or_else(|| params["name"].as_str()) {
        req = req.header("Mcp-Name", name);
    }
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let res = req.send(body.to_string()).unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let reader = BufReader::new(res.into_body().into_reader());
        for line in reader.lines().map_while(Result::ok) {
            let data = line.strip_prefix("data:").unwrap_or(&line).trim();
            if let Ok(message) = serde_json::from_str::<Value>(data) {
                if tx.send(message).is_err() {
                    return;
                }
            }
        }
    });
    rx
}

/// Waits until the gate opened its live watch on `fake`.
pub(super) fn wait_live(fake: &Mutex<Fake>) {
    for _ in 0..500 {
        if !fake.lock().unwrap().live.is_empty() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the gate never opened a live watch");
}

/// Checks the live watch asks after the tip, as the token's person, for the lifecycle kinds.
fn assert_watch(fake: &Mutex<Fake>) {
    let watches = fake.lock().unwrap().watches.clone();
    assert_eq!(watches.len(), 2, "{watches:?}");
    assert_eq!(watches[0]["follow"], false);
    assert!(
        watches[1].get("follow").is_none(),
        "the second watch follows"
    );
    assert_eq!(
        watches[1]["since"], TIP,
        "the listen starts at the newest event"
    );
    assert_eq!(watches[1]["identity"], "authgear:alice-sub");
    let kinds = watches[1]["kinds"].as_array().unwrap();
    for kind in [
        "idled",
        "agent_stalled",
        "agent_resumed",
        "setup_failed",
        "relaunch_capped",
        "started",
        "closed",
    ] {
        assert!(kinds.contains(&json!(kind)), "{kind} in {kinds:?}");
    }
}

/// The next message on `rx`; fails the test after 10 s.
pub(super) fn next(rx: &mpsc::Receiver<Value>) -> Value {
    rx.recv_timeout(Duration::from_secs(10)).unwrap()
}

#[test]
fn each_box_is_a_resource_that_reads_as_box_status() {
    let (world, _fake) = world();
    let token = token(&world, "moor:read");
    let listed = next(&post(&world, &token, "resources/list", json!({})));
    let uris: Vec<&str> = listed["result"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["uri"].as_str().unwrap())
        .collect();
    assert_eq!(uris, ["moor://box/repo/a", "moor://box/repo/b"], "{listed}");
    let read = next(&post(
        &world,
        &token,
        "resources/read",
        json!({"uri": "moor://box/repo/a"}),
    ));
    let contents = &read["result"]["contents"][0];
    assert_eq!(contents["mimeType"], "application/json", "{read}");
    let status: Value = serde_json::from_str(contents["text"].as_str().unwrap()).unwrap();
    assert_eq!(status["status"], "ready");
    assert_eq!(status["agent_said"]["text"], "Opened the PR.");
}

#[test]
fn listen_pushes_updates_for_subscribed_boxes_and_list_changes() {
    let (world, fake) = world();
    let token = token(&world, "moor:read");
    let params = json!({"notifications": {
        "resourcesListChanged": true,
        "resourceSubscriptions": ["moor://box/repo/a", "file:///etc/passwd"],
    }});
    let stream = post(&world, &token, "subscriptions/listen", params);
    let ack = next(&stream);
    assert_eq!(
        ack["method"], "notifications/subscriptions/acknowledged",
        "{ack}"
    );
    assert_eq!(
        ack["params"]["notifications"],
        json!({"resourcesListChanged": true, "resourceSubscriptions": ["moor://box/repo/a"]}),
        "only Box URIs are accepted"
    );
    wait_live(&fake);
    assert_watch(&fake);

    emit(&fake, 11, "repo/b", "idled");
    emit(&fake, 12, "repo/a", "idled");
    emit(&fake, 13, "repo/c", "started");
    let updated = next(&stream);
    assert_eq!(
        updated["method"], "notifications/resources/updated",
        "{updated}"
    );
    assert_eq!(
        updated["params"]["uri"], "moor://box/repo/a",
        "repo/b is not subscribed, so its idle sends nothing"
    );
    let listed = next(&stream);
    assert_eq!(
        listed["method"], "notifications/resources/list_changed",
        "{listed}"
    );
}

#[test]
fn listen_needs_the_read_scope() {
    let (world, _fake) = world();
    let token = token(&world, "moor:write");
    let params = json!({"notifications": {"resourcesListChanged": true}});
    let stream = post(&world, &token, "subscriptions/listen", params);
    let mut messages = vec![next(&stream)];
    while let Ok(message) = stream.recv_timeout(Duration::from_secs(2)) {
        messages.push(message);
    }
    let error = messages.iter().find(|m| m.get("error").is_some());
    let error = error.unwrap_or_else(|| panic!("no error in {messages:?}"));
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("moor:read"),
        "{error}"
    );
}

#[test]
fn a_client_that_leaves_ends_the_controller_watch() {
    let (world, fake) = world();
    let token = token(&world, "moor:read");
    let params = json!({"notifications": {"resourcesListChanged": true}});
    let stream = post(&world, &token, "subscriptions/listen", params);
    next(&stream);
    wait_live(&fake);
    drop(stream);
    // The reader thread notices the dropped receiver on the next message and hangs up.
    emit(&fake, 11, "repo/c", "started");
    for _ in 0..500 {
        if fake.lock().unwrap().ended == 1 {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the gate kept the controller watch after the client left");
}
