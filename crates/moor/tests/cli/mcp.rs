//! `moor mcp` serves MCP over stdio and relays each message to the gate's `/mcp`, signed in with
//! the shared sign-in.
#![allow(clippy::indexing_slicing, reason = "test assertions index JSON values")]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_testkit::oidc::{now, Issuer};
use serde_json::{json, Value};

/// One request the fake gate received.
#[derive(Debug, Clone)]
struct Seen {
    /// Header names lowercased.
    headers: HashMap<String, String>,
    /// The JSON-RPC message.
    body: Value,
}

/// A fake gate: answers `initialize`, `tools/list`, `tools/call` and `subscriptions/listen`
/// the way the stateless gate does, and refuses the tokens `refuse` names.
struct Gate {
    base: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Gate {
    fn start(refuse: fn(&str) -> bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let gate = Self {
            base: format!("http://{}", listener.local_addr().unwrap()),
            seen: Arc::new(Mutex::new(Vec::new())),
        };
        let seen = gate.seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let seen = seen.clone();
                std::thread::spawn(move || answer(stream, &seen, refuse));
            }
        });
        gate
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    /// Waits until the gate has received a `method` message.
    fn wait_for(&self, method: &str) {
        for _ in 0..1000 {
            if self.seen().iter().any(|s| s.body["method"] == method) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the gate never received {method}");
    }
}

fn answer(mut stream: TcpStream, seen: &Mutex<Vec<Seen>>, refuse: fn(&str) -> bool) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let Some((k, v)) = line.trim_end().split_once(": ") else {
            break;
        };
        headers.insert(k.to_ascii_lowercase(), v.to_owned());
    }
    let len: usize = headers["content-length"].parse().unwrap();
    let mut body = vec![0; len];
    reader.read_exact(&mut body).unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    let token = headers["authorization"]
        .trim_start_matches("Bearer ")
        .to_owned();
    seen.lock().unwrap().push(Seen {
        headers,
        body: body.clone(),
    });
    let id = body["id"].clone();
    let result = match body["method"].as_str().unwrap() {
        _ if refuse(&token) => return reply(&mut stream, "401 Unauthorized", "", ""),
        "notifications/initialized" => return reply(&mut stream, "202 Accepted", "", ""),
        "subscriptions/listen" => return listen(&mut stream, &id),
        "initialize" => json!({"protocolVersion": "2025-11-25", "capabilities": {"tools": {}},
            "serverInfo": {"name": "moor-gate", "version": "1"}}),
        "tools/list" => {
            json!({"tools": [{"name": "list_boxes", "inputSchema": {"type": "object"}}]})
        }
        "tools/call" => json!({"content": [{"type": "text", "text": "no boxes"}]}),
        other => panic!("unexpected {other}"),
    };
    let message = json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string();
    reply(&mut stream, "200 OK", "application/json", &message);
}

/// Writes one response; `moor mcp` exits on stdin's end without waiting for answers still in
/// flight, so the peer may already be gone.
fn reply(stream: &mut TcpStream, status: &str, kind: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// Opens an event stream with one notification and keeps it open, as a live listen does.
fn listen(stream: &mut TcpStream, id: &Value) {
    let ack = json!({"jsonrpc": "2.0", "id": id, "result": {}, "_note": "ack"});
    let event = json!({"jsonrpc": "2.0", "method": "notifications/resources/updated",
        "params": {"uri": "moor://box/app/fix"}});
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n\
         : keepalive\n\nevent: message\ndata: {event}\n\n"
    )
    .unwrap();
    stream.flush().unwrap();
    std::thread::sleep(Duration::from_secs(20));
    let _ = write!(stream, "data: {ack}\n\n");
}

/// Saves a sign-in to `gate`, its tokens issued by `issuer`, with access token `access-0`.
fn signed_in(state: &Path, gate: &Gate, issuer: &Issuer) {
    let creds = json!({"servers": {gate.base.clone(): {
        "server": gate.base, "issuer": issuer.base, "client_id": issuer.client_id,
        "token_endpoint": format!("{}/token", issuer.base), "refresh_token": "refresh-0",
        "access_token": "access-0", "expires_at": now() + 3600}}});
    std::fs::create_dir_all(state.join("moor")).unwrap();
    std::fs::write(state.join("moor/credentials.json"), creds.to_string()).unwrap();
}

/// A running `moor mcp` and the lines of its stdout.
struct Mcp {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
}

impl Mcp {
    fn start(state: &Path, gate: &Gate) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_moor"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", state.join("home"))
            .env("XDG_STATE_HOME", state)
            .env("MOOR_SERVER", format!("{}/mcp", gate.base))
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in stdout.lines() {
                let _ = tx.send(line.unwrap());
            }
        });
        Self {
            stdin: child.stdin.take(),
            child,
            lines,
        }
    }

    fn send(&mut self, message: &Value) {
        writeln!(self.stdin.as_mut().unwrap(), "{message}").unwrap();
    }

    /// The next stdout line, which must be one JSON-RPC message.
    fn next(&self) -> Value {
        let line = self.lines.recv_timeout(Duration::from_secs(10)).unwrap();
        let message: Value = serde_json::from_str(&line).unwrap_or_else(|_| panic!("{line}"));
        assert_eq!(message["jsonrpc"], "2.0", "{line}");
        message
    }

    fn ask(&mut self, message: &Value) -> Value {
        self.send(message);
        self.next()
    }

    /// Closes stdin; `moor mcp` exits cleanly and wrote nothing more.
    fn finish(mut self) {
        drop(self.stdin.take());
        assert!(self.child.wait().unwrap().success());
        assert!(self.lines.recv_timeout(Duration::from_secs(5)).is_err());
    }
}

fn request(id: u64, method: &str, params: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn initialize() -> Value {
    request(
        1,
        "initialize",
        &json!({"protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": {"name": "agent", "version": "1"}}),
    )
}

#[test]
fn tools_list_round_trips_with_the_saved_token() {
    let issuer = Issuer::start("moor-cli");
    let gate = Gate::start(|_| false);
    let dir = tempfile::tempdir().unwrap();
    signed_in(dir.path(), &gate, &issuer);
    let mut mcp = Mcp::start(dir.path(), &gate);

    let init = mcp.ask(&initialize());
    assert_eq!(init["id"], 1);
    assert_eq!(init["result"]["protocolVersion"], "2025-11-25");
    mcp.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    // `moor mcp` relays each line on its own thread, so it may send the next request first.
    gate.wait_for("notifications/initialized");
    let tools = mcp.ask(&request(2, "tools/list", &json!({})));
    assert_eq!(
        tools,
        json!({"jsonrpc": "2.0", "id": 2, "result": {"tools": [
            {"name": "list_boxes", "inputSchema": {"type": "object"}}]}})
    );
    mcp.finish();

    let seen = gate.seen();
    assert_eq!(seen.len(), 3);
    assert!(seen
        .iter()
        .all(|s| s.headers["authorization"] == "Bearer access-0"));
    let list = &seen[2];
    assert_eq!(list.body["method"], "tools/list");
    assert_eq!(list.headers["mcp-protocol-version"], "2025-11-25");
    assert_eq!(list.headers["mcp-method"], "tools/list");
}

#[test]
fn a_refused_token_is_refreshed_once_and_the_call_retried() {
    let issuer = Issuer::start("moor-cli");
    let gate = Gate::start(|token| token == "access-0");
    let dir = tempfile::tempdir().unwrap();
    signed_in(dir.path(), &gate, &issuer);
    let mut mcp = Mcp::start(dir.path(), &gate);

    let tools = mcp.ask(&request(2, "tools/list", &json!({})));
    assert_eq!(tools["result"]["tools"][0]["name"], "list_boxes", "{tools}");
    mcp.finish();

    assert_eq!(issuer.refreshes.load(Ordering::SeqCst), 1);
    let tokens: Vec<String> = gate
        .seen()
        .iter()
        .map(|s| s.headers["authorization"].clone())
        .collect();
    assert_eq!(tokens.len(), 2);
    assert_eq!(tokens[0], "Bearer access-0");
    assert_ne!(tokens[1], "Bearer access-0");
}

#[test]
fn a_token_refused_after_refreshing_says_to_log_in() {
    let issuer = Issuer::start("moor-cli");
    let gate = Gate::start(|_| true);
    let dir = tempfile::tempdir().unwrap();
    signed_in(dir.path(), &gate, &issuer);
    let mut mcp = Mcp::start(dir.path(), &gate);

    let answer = mcp.ask(&request(2, "tools/list", &json!({})));
    assert_eq!(answer["id"], 2);
    let message = answer["error"]["message"].as_str().unwrap();
    assert!(
        message.contains(&format!("run `moor login --server {}`", gate.base)),
        "{message}"
    );
    mcp.finish();
    assert_eq!(gate.seen().len(), 2);
    assert_eq!(issuer.refreshes.load(Ordering::SeqCst), 1);
}

#[test]
fn without_a_sign_in_it_starts_and_each_request_says_how_to_sign_in() {
    let gate = Gate::start(|_| false);
    let dir = tempfile::tempdir().unwrap();
    let mut mcp = Mcp::start(dir.path(), &gate);

    let base = &gate.base;
    for (id, message) in [(1, initialize()), (2, request(2, "tools/list", &json!({})))] {
        let answer = mcp.ask(&message);
        assert_eq!(answer["id"], id);
        assert_eq!(
            answer["error"]["message"],
            format!("not signed in to {base}; run `moor login --server {base}`")
        );
    }
    mcp.finish();
    assert!(gate.seen().is_empty());
}

#[test]
fn a_listen_stream_relays_each_event_as_it_arrives_while_calls_go_on() {
    let issuer = Issuer::start("moor-cli");
    let gate = Gate::start(|_| false);
    let dir = tempfile::tempdir().unwrap();
    signed_in(dir.path(), &gate, &issuer);
    let mut mcp = Mcp::start(dir.path(), &gate);
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "agent", "version": "1"},
        "io.modelcontextprotocol/clientCapabilities": {}});

    let params = json!({"_meta": meta,
        "notifications": {"resourceSubscriptions": ["moor://box/app/fix"]}});
    mcp.send(&request(7, "subscriptions/listen", &params));
    let event = mcp.next();
    assert_eq!(event["method"], "notifications/resources/updated");
    assert_eq!(event["params"]["uri"], "moor://box/app/fix");

    let call = mcp.ask(&request(
        8,
        "tools/call",
        &json!({"_meta": meta, "name": "list_boxes", "arguments": {}}),
    ));
    assert_eq!(call["result"]["content"][0]["text"], "no boxes", "{call}");

    let seen = gate.seen();
    let listen = &seen[0];
    assert_eq!(listen.headers["mcp-protocol-version"], "2026-07-28");
    assert_eq!(listen.headers["mcp-method"], "subscriptions/listen");
    assert_eq!(listen.body["params"], params);
    let call = &seen[1];
    assert_eq!(call.headers["mcp-name"], "list_boxes");
    assert_eq!(call.headers["mcp-protocol-version"], "2026-07-28");
    drop(mcp.stdin.take());
    assert!(mcp.child.wait().unwrap().success());
}
