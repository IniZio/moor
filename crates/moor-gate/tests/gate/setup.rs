//! The webhook setup page at `/webhooks/<id>/setup` and the link `add_webhook` gives to it,
//! against the testkit issuer and a fake controller that keeps webhooks in memory.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixListener;
use std::sync::{Arc, Mutex};

use moor_core::response::{ErrorKind, ErrorView, Response};
use moor_gate::{Gate, Login, Mcp, Verifier};
use moor_testkit::oidc::{now, Issuer};
use oauth2::url::form_urlencoded;
use serde_json::{json, Value};

use super::login::{assert_page_headers, csrf, get, send, sign_in, Reply};
use super::CLIENT_ID;

const KEY: &str = "4f2c9a0d8b7e6f5a4c3b2a1908f7e6d5c4b3a29180f7e6d5c4b3a29180f7e6d5";
const HEADER_VALUE: &str = "Bearer s3cret-<grok>&token";

/// One webhook the fake controller keeps.
struct Hook {
    owner: String,
    url: String,
    header: Option<String>,
    active: bool,
}

struct World {
    addr: String,
    issuer: Issuer,
    /// Every request line the controller got.
    seen: Arc<Mutex<Vec<String>>>,
    _dir: tempfile::TempDir,
}

fn refuse(message: &str) -> Result<Value, String> {
    Err(message.to_owned())
}

/// What the controller answers `cmd` from `who`, keeping webhooks in `hooks`.
fn answer(hooks: &mut Vec<Hook>, who: &str, inner: &Value) -> Result<Value, String> {
    let cmd = inner["cmd"].as_str().unwrap_or_default();
    let id = inner["id"].as_i64().unwrap_or_default();
    let index = usize::try_from(id - 1).ok();
    let mine = index.filter(|i| hooks.get(*i).is_some_and(|h| h.owner == who));
    match cmd {
        "webhook-add" => {
            hooks.push(Hook {
                owner: who.to_owned(),
                url: inner["url"].as_str().unwrap().to_owned(),
                header: inner["header"].as_str().map(str::to_owned),
                active: false,
            });
            Ok(json!({"id": hooks.len(), "state": "waiting_for_secret"}))
        }
        "webhook-list" => Ok(json!({"webhooks": hooks.iter().enumerate()
            .filter(|(_, h)| h.owner == who)
            .map(|(i, h)| json!({"id": i + 1, "url": h.url, "events": ["box_ready"],
                "state": if h.active { "active" } else { "waiting_for_secret" },
                "header": h.header, "created_at": 1, "last_ok_at": null,
                "failing_since": null, "last_error": null}))
            .collect::<Vec<_>>()})),
        "webhook-setup-show" => {
            let Some(h) = mine.map(|i| &hooks[i]) else {
                return refuse("no such webhook");
            };
            let mut value = json!({"id": id, "url": h.url, "header": h.header,
                "state": if h.active { "active" } else { "waiting_for_secret" }});
            if !h.active {
                value["key"] = json!(KEY);
            }
            Ok(value)
        }
        "webhook-setup-finish" => {
            let Some(h) = mine.map(|i| &mut hooks[i]) else {
                return refuse("no such webhook");
            };
            if h.active {
                return refuse("this webhook is already set up");
            }
            if h.header.is_some() != inner["header_value"].is_string() {
                return refuse("give the <header> value exactly when the webhook has a header");
            }
            h.active = true;
            Ok(json!({"id": id, "state": "active"}))
        }
        _ => Ok(Value::Null),
    }
}

fn serve_controller(path: &std::path::Path) -> Arc<Mutex<Vec<String>>> {
    let listener = UnixListener::bind(path).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let hooks = Arc::new(Mutex::new(Vec::new()));
    let kept = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (seen, hooks) = (kept.clone(), hooks.clone());
            std::thread::spawn(move || {
                let mut reply = stream.try_clone().unwrap();
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let frame: Value = serde_json::from_str(&line).unwrap();
                    if frame["frame"] == "hello" {
                        writeln!(reply, "{line}").unwrap();
                        continue;
                    }
                    seen.lock().unwrap().push(line.clone());
                    let who = frame["request"]["identity"].as_str().unwrap_or_default();
                    let inner = &frame["request"]["request"];
                    let answered = answer(&mut hooks.lock().unwrap(), who, inner);
                    let envelope = match answered {
                        Ok(value) => Response::ok("x", Some(value)),
                        Err(message) => Response::err(
                            "x",
                            ErrorView {
                                kind: ErrorKind::Usage,
                                message,
                                hints: vec![],
                                context: vec![],
                                detail: None,
                            },
                        ),
                    };
                    let out = json!({"frame": "response", "id": frame["id"], "envelope": envelope});
                    writeln!(reply, "{out}").unwrap();
                }
            });
        }
    });
    seen
}

/// A gate with `/mcp`, and with browser sign-in when `login` is set.
fn world(login: bool) -> World {
    let issuer = Issuer::start(CLIENT_ID);
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("authgear.sock");
    let seen = serve_controller(&socket);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
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
    std::thread::spawn(move || moor_gate::serve_with(&listener, &gate, login.as_ref()));
    World {
        addr,
        issuer,
        seen,
        _dir: dir,
    }
}

impl World {
    /// A `/mcp` token for `sub` with both scopes.
    fn token(&self, sub: &str) -> String {
        self.issuer.sign(&json!({"iss": self.issuer.base,
            "aud": format!("http://{}/mcp", self.addr), "sub": sub, "exp": now() + 600,
            "scope": "moor:read moor:write", "email": format!("{sub}@example.com"),
            "email_verified": true}))
    }

    /// The `structuredContent` of tool `tool` called by `sub` with `args`.
    fn call(&self, sub: &str, tool: &str, args: Value) -> Value {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into();
        let token = self.token(sub);
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
        let (reply, _) = post(
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": {"name": tool, "arguments": args}}),
            session.as_deref(),
        );
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        reply["result"]["structuredContent"].clone()
    }

    /// Adds a webhook wanting an `Authorization` header for `sub`: its setup path.
    fn add(&self, sub: &str) -> String {
        let added = self.call(
            sub,
            "add_webhook",
            json!({"url": "https://grok.example/hook", "header": "Authorization"}),
        );
        let url = added["setup_url"].as_str().unwrap();
        url.strip_prefix(&format!("http://{}", self.addr))
            .unwrap()
            .to_owned()
    }

    fn post(&self, path: &str, cookie: Option<&str>, form: &[(&str, &str)]) -> Reply {
        let body = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(form)
            .finish();
        let cookie = cookie.map_or_else(String::new, |c| format!("Cookie: {c}\r\n"));
        let extra = format!("Content-Type: application/x-www-form-urlencoded\r\n{cookie}");
        send(&self.addr, "POST", path, &extra, &body)
    }

    fn seen(&self) -> String {
        self.seen.lock().unwrap().join("\n")
    }
}

#[test]
fn add_webhook_gives_a_setup_link_and_never_a_key() {
    let w = world(true);
    let added = w.call(
        "alice-sub",
        "add_webhook",
        json!({"url": "https://grok.example/hook", "header": "Authorization"}),
    );
    let link = format!("http://{}/webhooks/1/setup", w.addr);
    assert_eq!(added["setup_url"], json!(link), "{added}");
    assert!(added.get("key").is_none(), "{added}");
    assert!(!added.to_string().contains(KEY), "{added}");
    let note = added["note"].as_str().unwrap();
    for want in [
        "browser",
        "signing key",
        "header value",
        "Never paste secrets",
    ] {
        assert!(note.contains(want), "{want} missing from {note}");
    }
    assert!(
        w.seen().contains(r#""header":"Authorization""#),
        "{}",
        w.seen()
    );
    let listed = w.call("alice-sub", "list_webhooks", json!({}));
    let row = &listed["webhooks"][0];
    assert!(
        row["health"]
            .as_str()
            .unwrap()
            .contains("waiting for secret"),
        "{row}"
    );
    assert_eq!(row["setup_url"], json!(link), "{row}");
}

#[test]
fn the_setup_page_sends_a_signed_out_browser_to_sign_in() {
    let w = world(true);
    let path = w.add("alice-sub");
    let page = get(&w.addr, &path, None);
    assert_eq!(page.status, 302, "{}", page.body);
    assert_eq!(page.location(), format!("/auth/login?return={path}"));
}

#[test]
fn the_waiting_page_shows_the_key_and_asks_for_the_header_value() {
    let w = world(true);
    let path = w.add("alice-sub");
    let cookie = sign_in(&w.addr);
    let page = get(&w.addr, &path, Some(&cookie));
    assert_eq!(page.status, 200, "{}", page.body);
    assert_page_headers(&page);
    for want in [
        &format!("<code>{KEY}</code>"),
        "Save this key now; it is shown only while setup is unfinished. The receiver checks X-Moor-Signature with it.",
        "https://grok.example/hook",
        "Authorization value",
        "type=\"password\"",
        "name=\"header_value\"",
        "Finish setup",
    ] {
        assert!(page.body.contains(want), "{want} missing from {}", page.body);
    }
    assert!(!csrf(&page).is_empty());
}

/// Adds alice's webhook, signs her in and finishes its setup with [`HEADER_VALUE`]: its
/// setup path, her cookie and the finished page.
fn finished(w: &World) -> (String, String, Reply) {
    let path = w.add("alice-sub");
    let cookie = sign_in(&w.addr);
    let token = csrf(&get(&w.addr, &path, Some(&cookie)));
    let form = [("csrf", token.as_str()), ("header_value", HEADER_VALUE)];
    let done = w.post(&path, Some(&cookie), &form);
    (path, cookie, done)
}

#[test]
fn a_refused_finish_shows_the_escaped_reason_and_keeps_waiting() {
    let w = world(true);
    let path = w.add("alice-sub");
    let cookie = sign_in(&w.addr);
    let token = csrf(&get(&w.addr, &path, Some(&cookie)));
    let refused = w.post(&path, Some(&cookie), &[("csrf", &token)]);
    assert_eq!(refused.status, 400, "{}", refused.body);
    assert!(
        refused.body.contains("give the &lt;header&gt; value"),
        "{}",
        refused.body
    );
    assert!(refused.body.contains(KEY), "{}", refused.body);
}

#[test]
fn finishing_with_the_header_value_activates_it_and_never_echoes_the_value() {
    let w = world(true);
    let (path, cookie, done) = finished(&w);
    assert_eq!(done.status, 200, "{}", done.body);
    assert!(
        done.body
            .contains("Set up. Deliveries start with the next event."),
        "{}",
        done.body
    );
    let sent = r#""header_value":"Bearer s3cret-<grok>&token""#;
    assert!(w.seen().contains(sent), "{}", w.seen());
    let active = get(&w.addr, &path, Some(&cookie));
    assert_eq!(active.status, 200, "{}", active.body);
    assert!(active.body.contains("set up"), "{}", active.body);
    assert!(!active.body.contains(KEY), "{}", active.body);
    assert!(!active.body.contains("<form"), "{}", active.body);
    for body in [&done.body, &active.body] {
        assert!(!body.contains("s3cret"), "{body}");
    }
}

#[test]
fn a_finished_webhook_lists_as_active_without_a_setup_link() {
    let w = world(true);
    let (_, _, done) = finished(&w);
    assert_eq!(done.status, 200, "{}", done.body);
    let listed = w.call("alice-sub", "list_webhooks", json!({}));
    let row = &listed["webhooks"][0];
    assert!(row["health"].as_str().unwrap().contains("active"), "{row}");
    assert!(row.get("setup_url").is_none(), "{row}");
    assert!(!listed.to_string().contains("s3cret"), "{listed}");
}

#[test]
fn finishing_needs_the_session_and_its_csrf_token() {
    let w = world(true);
    let path = w.add("alice-sub");
    let cookie = sign_in(&w.addr);
    let value = ("header_value", HEADER_VALUE);
    for (cookie, form) in [
        (Some(cookie.as_str()), vec![value]),
        (Some(cookie.as_str()), vec![("csrf", "forged"), value]),
        (None, vec![("csrf", "forged"), value]),
    ] {
        let reply = w.post(&path, cookie, &form);
        assert_eq!(reply.status, 403, "{}", reply.body);
        assert!(!reply.body.contains("s3cret"), "{}", reply.body);
    }
    assert!(!w.seen().contains("webhook-setup-finish"), "{}", w.seen());
}

#[test]
fn another_person_s_webhook_and_a_bad_id_are_not_found() {
    let w = world(true);
    let bobs = w.add("bob-sub");
    let cookie = sign_in(&w.addr);
    for path in [
        bobs.as_str(),
        "/webhooks/0/setup",
        "/webhooks/+1/setup",
        "/webhooks/x/setup",
    ] {
        let page = get(&w.addr, path, Some(&cookie));
        assert_eq!(page.status, 404, "{path}: {}", page.body);
        assert!(!page.body.contains(KEY), "{}", page.body);
    }
}

#[test]
fn without_browser_sign_in_there_is_no_link_and_no_page() {
    let w = world(false);
    let added = w.call(
        "alice-sub",
        "add_webhook",
        json!({"url": "https://grok.example/hook"}),
    );
    assert!(added.get("setup_url").is_none(), "{added}");
    let note = added["note"].as_str().unwrap();
    assert!(note.contains("browser sign-in"), "{note}");
    assert!(!note.contains("http"), "{note}");
    let page = get(&w.addr, "/webhooks/1/setup", None);
    assert_eq!(page.status, 404);
}
