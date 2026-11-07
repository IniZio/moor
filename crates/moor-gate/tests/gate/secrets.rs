//! The secrets page at `/me/secrets` and the `start_box` refusal that links to it, against the
//! testkit issuer and a fake controller that keeps each person's secrets in memory.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixListener;
use std::sync::{Arc, Mutex};

use moor_core::response::{ErrorDetail, ErrorKind, ErrorView, Response};
use moor_gate::{Gate, Login, Mcp, Verifier};
use moor_testkit::oidc::{now, Issuer};
use oauth2::url::form_urlencoded;
use serde_json::{json, Value};

use super::login::{assert_page_headers, csrf, get, send, sign_in, Reply};
use super::CLIENT_ID;

const VALUE: &str = "sk-ant-oat01-s3cret<&>";
const STORED: &str = "ghp_alreadyStored";

/// Each enrolled identity's secrets, by name.
type Vault = HashMap<String, BTreeMap<String, String>>;

struct World {
    addr: String,
    issuer: Issuer,
    /// Every request the controller got, `secret-set` values redacted.
    seen: Arc<Mutex<Vec<Value>>>,
    vault: Arc<Mutex<Vault>>,
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
        "start-needs" | "start" => needs(vault, who, inner),
        _ => Response::ok("x", None),
    }
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
fn world(login: bool, first: &'static [(&'static str, &'static str)]) -> World {
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
    fn post(&self, cookie: Option<&str>, form: &[(&str, &str)]) -> Reply {
        let body = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(form)
            .finish();
        let cookie = cookie.map_or_else(String::new, |c| format!("Cookie: {c}\r\n"));
        let extra = format!("Content-Type: application/x-www-form-urlencoded\r\n{cookie}");
        send(&self.addr, "POST", "/me/secrets", &extra, &body)
    }

    /// The `cmd` of every request the controller got, in order.
    fn cmds(&self) -> Vec<String> {
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
fn the_secrets_page_sends_a_signed_out_browser_to_sign_in() {
    let w = world(true, &[]);
    let page = get(&w.addr, "/me/secrets", None);
    assert_eq!(page.status, 302, "{}", page.body);
    assert_eq!(page.location(), "/auth/login?return=/me/secrets");
    assert!(w.cmds().is_empty(), "{:?}", w.cmds());
}

#[test]
fn the_secrets_page_lists_set_and_missing_names_and_never_a_value() {
    let w = world(true, &[("github", STORED)]);
    let cookie = sign_in(&w.addr);
    let page = get(&w.addr, "/me/secrets", Some(&cookie));
    assert_eq!(page.status, 200, "{}", page.body);
    assert_page_headers(&page);
    for want in [
        "<code>claude</code>: not set",
        "<code>github</code>: set",
        "claude setup-token",
        "gh auth token",
        "type=\"password\"",
        "name=\"value\"",
        "method=\"post\" action=\"/me/secrets\"",
    ] {
        assert!(
            page.body.contains(want),
            "{want} missing from {}",
            page.body
        );
    }
    assert!(!page.body.contains(STORED), "{}", page.body);
    assert!(!csrf(&page).is_empty());
}

#[test]
fn saving_a_secret_enrolls_then_stores_it_and_never_echoes_it() {
    let w = world(true, &[]);
    let cookie = sign_in(&w.addr);
    let token = csrf(&get(&w.addr, "/me/secrets", Some(&cookie)));
    let saved = w.post(
        Some(&cookie),
        &[("csrf", &token), ("name", "claude"), ("value", VALUE)],
    );
    assert_eq!(saved.status, 200, "{}", saved.body);
    assert_page_headers(&saved);
    assert!(
        saved.body.contains("Saved <code>claude</code>."),
        "{}",
        saved.body
    );
    assert!(
        saved.body.contains("<code>claude</code>: set"),
        "{}",
        saved.body
    );
    assert!(!saved.body.contains("s3cret"), "{}", saved.body);
    let cmds = w.cmds();
    let set = cmds.iter().position(|c| c == "secret-set").unwrap();
    assert!(cmds[..set].contains(&"enroll".to_owned()), "{cmds:?}");
    let (who, mine) = w.vault.lock().unwrap().clone().into_iter().next().unwrap();
    assert!(who.contains("alice-sub"), "{who}");
    assert_eq!(mine["claude"], VALUE);
    let logged = format!("{:?}", w.seen.lock().unwrap());
    assert!(!logged.contains("s3cret"), "{logged}");
}

#[test]
fn saving_needs_the_session_and_its_csrf_token() {
    let w = world(true, &[]);
    let cookie = sign_in(&w.addr);
    let value = ("value", VALUE);
    let name = ("name", "claude");
    for (cookie, form) in [
        (Some(cookie.as_str()), vec![name, value]),
        (Some(cookie.as_str()), vec![("csrf", "forged"), name, value]),
        (None, vec![("csrf", "forged"), name, value]),
    ] {
        let reply = w.post(cookie, &form);
        assert_eq!(reply.status, 403, "{}", reply.body);
        assert!(!reply.body.contains("s3cret"), "{}", reply.body);
    }
    assert!(
        !w.cmds().contains(&"secret-set".to_owned()),
        "{:?}",
        w.cmds()
    );
}

#[test]
fn a_bad_name_or_an_empty_value_is_refused_without_storing() {
    let w = world(true, &[]);
    let cookie = sign_in(&w.addr);
    let token = csrf(&get(&w.addr, "/me/secrets", Some(&cookie)));
    for form in [
        vec![
            ("csrf", token.as_str()),
            ("name", "Not A Name"),
            ("value", VALUE),
        ],
        vec![("csrf", token.as_str()), ("name", "claude"), ("value", "")],
    ] {
        let reply = w.post(Some(&cookie), &form);
        assert_eq!(reply.status, 400, "{}", reply.body);
        assert!(!reply.body.contains("s3cret"), "{}", reply.body);
    }
    assert!(
        !w.cmds().contains(&"secret-set".to_owned()),
        "{:?}",
        w.cmds()
    );
}

#[test]
fn start_box_refused_for_missing_secrets_links_the_secrets_page() {
    let w = world(true, &[]);
    let reply = w.call(
        "alice-sub",
        "start_box",
        json!({"repo": "acme/web", "wait": true}),
    );
    assert_eq!(reply["result"]["isError"], true, "{reply}");
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains(&format!("http://{}/me/secrets", w.addr)),
        "{text}"
    );
    assert!(text.contains("claude, github"), "{text}");
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
    assert_eq!(get(&w.addr, "/me/secrets", None).status, 404);
}

#[test]
fn a_start_that_does_not_wait_links_the_secrets_page_too() {
    let w = world(true, &[]);
    let reply = w.call("alice-sub", "start_box", json!({"repo": "acme/web"}));
    assert_eq!(reply["result"]["isError"], true, "{reply}");
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("/me/secrets"), "{text}");
}
