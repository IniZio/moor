//! The `start_box` refusal that links to the account page, against the
//! testkit issuer and a fake controller that keeps each person's secrets in memory.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixListener;
use std::sync::{Arc, Mutex};

use moor_core::response::{ErrorDetail, ErrorKind, ErrorView, Response};
use moor_gate::{Gate, Login, Mcp, Verifier};
use moor_testkit::oidc::{now, Issuer};
use serde_json::{json, Value};

use super::login::get;
use super::CLIENT_ID;

/// Each enrolled identity's secrets, by name.
pub(super) type Vault = HashMap<String, BTreeMap<String, String>>;

pub(super) struct World {
    pub(super) addr: String,
    pub(super) issuer: Issuer,
    /// Every request the controller got, `secret-set` values redacted.
    pub(super) seen: Arc<Mutex<Vec<Value>>>,
    pub(super) vault: Arc<Mutex<Vault>>,
    _dir: tempfile::TempDir,
}

/// A refusal saying `message`.
fn err(message: &str, detail: Option<ErrorDetail>) -> Response {
    Response::err(
        "x",
        ErrorView {
            kind: ErrorKind::Refused,
            message: message.to_owned(),
            hints: vec![],
            context: vec![],
            detail,
        },
    )
}

/// What the controller answers `inner` from `who`; `first` is what a new user starts with.
fn answer(vault: &mut Vault, first: &[(&str, &str)], who: &str, inner: &Value) -> Response {
    match inner["cmd"].as_str().unwrap_or_default() {
        "enroll" => {
            vault.entry(who.to_owned()).or_insert_with(|| {
                first
                    .iter()
                    .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
                    .collect()
            });
            Response::ok("x", Some(json!({"identity": who})))
        }
        "secret-set" => vault.get_mut(who).map_or_else(
            || err("no such user; enroll first", None),
            |mine| {
                let name = inner["name"].as_str().unwrap().to_owned();
                mine.insert(name, inner["value"].as_str().unwrap().to_owned());
                Response::ok("x", Some(json!({"changed": true})))
            },
        ),
        "secret-names" => {
            let names: Vec<&String> = vault
                .get(who)
                .map(|m| m.keys().collect())
                .unwrap_or_default();
            Response::ok("x", Some(json!({"names": names})))
        }
        "node-list" => Response::ok("x", Some(nodes())),
        "start-needs" | "start" => needs(vault, who, inner),
        _ => Response::ok("x", None),
    }
}

/// The answer to `node-list`: one personal node, online with room.
fn nodes() -> Value {
    json!({"nodes": [{"name": "hv", "scope": "personal", "runtime": "cloud-hypervisor",
        "online": true, "used": 1, "waiting": 0, "max_boxes": 4, "labels": []}], "at": 100})
}

/// The answer to `start-needs`, or the refusal of a `start` missing secrets.
fn needs(vault: &Vault, who: &str, inner: &Value) -> Response {
    let needs = ["claude", "github"];
    let mine = vault.get(who);
    let missing: Vec<&str> = needs
        .into_iter()
        .filter(|n| !mine.is_some_and(|m| m.contains_key(*n)))
        .collect();
    let value = json!({"identity": who, "user": mine.map(|_| 1), "needs": needs,
        "missing": missing});
    if inner["cmd"] == "start-needs" {
        return Response::ok("x", Some(value));
    }
    let steps: Vec<String> = missing
        .iter()
        .map(|n| format!("moor user secret set {n}"))
        .collect();
    let mut refused = err(
        &format!("log in first: {}", steps.join(", then ")),
        Some(ErrorDetail::LoginRequired),
    );
    refused.value = Some(value);
    refused
}

/// A gate with `/mcp`, with browser sign-in when `login` is set; each new user starts with
/// the secrets in `first`.
pub(super) fn world(login: bool, first: &'static [(&'static str, &'static str)]) -> World {
    let issuer = Issuer::start(CLIENT_ID);
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("authgear.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let (seen, vault) = (Arc::default(), Arc::default());
    let (kept, kept_vault): (Arc<Mutex<Vec<Value>>>, Arc<Mutex<Vault>>) =
        (Arc::clone(&seen), Arc::clone(&vault));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (seen, vault) = (kept.clone(), kept_vault.clone());
            std::thread::spawn(move || {
                let mut reply = stream.try_clone().unwrap();
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let frame: Value = serde_json::from_str(&line).unwrap();
                    if frame["frame"] == "hello" {
                        writeln!(reply, "{line}").unwrap();
                        continue;
                    }
                    let who = frame["request"]["identity"].as_str().unwrap_or_default();
                    let inner = &frame["request"]["request"];
                    let mut logged = inner.clone();
                    if logged["cmd"] == "secret-set" {
                        logged["value"] = json!("<redacted>");
                    }
                    seen.lock()
                        .unwrap()
                        .push(json!({"who": who, "request": logged}));
                    let envelope = answer(&mut vault.lock().unwrap(), first, who, inner);
                    let out = json!({"frame": "response", "id": frame["id"], "envelope": envelope});
                    writeln!(reply, "{out}").unwrap();
                }
            });
        }
    });
    let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = tcp.local_addr().unwrap().to_string();
    let public = login.then(|| format!("http://{addr}"));
    let mcp = Mcp::with_setup(
        &format!("http://{addr}/mcp"),
        moor_client::Client::new(&socket),
        public.as_deref(),
    )
    .unwrap();
    let login = public.map(|p| Arc::new(Login::new(&p).unwrap()));
    let gate = Arc::new(Gate {
        controller: moor_client::Client::new(socket),
        mcp: Some(mcp),
        verifier: Verifier::new(issuer.base.clone(), CLIENT_ID.to_owned(), None),
    });
    std::thread::spawn(move || moor_gate::serve_with(&tcp, &gate, login.as_ref()));
    World {
        addr,
        issuer,
        seen,
        vault,
        _dir: dir,
    }
}

impl World {
    /// The `cmd` of every request the controller got, in order.
    pub(super) fn cmds(&self) -> Vec<String> {
        let seen = self.seen.lock().unwrap();
        seen.iter()
            .map(|r| r["request"]["cmd"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    /// The JSON-RPC reply to tool `tool` called by `sub` with `args`.
    fn call(&self, sub: &str, tool: &str, args: Value) -> Value {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into();
        let token = self.issuer.sign(&json!({"iss": self.issuer.base,
            "aud": format!("http://{}/mcp", self.addr), "sub": sub, "exp": now() + 600,
            "scope": "moor:read moor:write", "email": format!("{sub}@example.com"),
            "email_verified": true}));
        let post = |body: Value, session: Option<&str>| {
            let mut req = agent
                .post(format!("http://{}/mcp", self.addr))
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
            (serde_json::from_str(&text).unwrap_or(Value::Null), session)
        };
        let (_, session): (Value, _) = post(
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "test", "version": "0"}}}),
            None,
        );
        post(
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            session.as_deref(),
        );
        post(
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": {"name": tool, "arguments": args}}),
            session.as_deref(),
        )
        .0
    }
}

#[test]
fn start_box_refused_for_missing_secrets_links_the_account_page() {
    let w = world(true, &[]);
    let reply = w.call(
        "alice-sub",
        "start_box",
        json!({"repo": "acme/web", "wait": true}),
    );
    assert_eq!(reply["result"]["isError"], true, "{reply}");
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains(&format!("sign in at http://{}/ and", w.addr)),
        "{text}"
    );
    assert!(text.contains("claude, github"), "{text}");
    assert!(!text.contains("/me/secrets"), "{text}");
    assert!(!text.contains("moor user secret set"), "{text}");
}

#[test]
fn without_browser_sign_in_the_refusal_keeps_the_controller_s_words() {
    let w = world(false, &[]);
    let reply = w.call(
        "alice-sub",
        "start_box",
        json!({"repo": "acme/web", "wait": true}),
    );
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("moor user secret set claude"), "{text}");
    assert_eq!(get(&w.addr, "/", None).status, 404);
}

#[test]
fn a_start_that_does_not_wait_links_the_account_page_too() {
    let w = world(true, &[]);
    let reply = w.call("alice-sub", "start_box", json!({"repo": "acme/web"}));
    assert_eq!(reply["result"]["isError"], true, "{reply}");
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains(&format!("http://{}/ and", w.addr)), "{text}");
}
