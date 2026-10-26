#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use moor_core::config::{OAuthClient, UserConfig};
use moor_core::request::UserCmd;
use moor_core::seam::clock::{Clock, Ts};
use moor_core::user::{SecretName, UserId};
use moor_core::ErrorKind;
use oauth2::url::Url;
use serde_json::{json, Value};

use crate::ops::users::run;
use crate::ops::users::tests::{fixture, id, Fixture};
use crate::testing::clock::FakeClock;
use crate::vault::Vault;

/// A request the fake servers saw: method, path, body.
type Seen = Arc<Mutex<Vec<(String, String, String)>>>;

/// A fake MCP resource server and authorization server on one loopback port.
struct Fake {
    /// `http://127.0.0.1:<port>`.
    base: String,
    /// Every request so far.
    seen: Seen,
    /// How the fake answers.
    knobs: Knobs,
}

/// What a test changes about the fake's answers.
#[derive(Clone, Default)]
struct Knobs {
    /// Fields that replace the authorization server metadata's.
    meta: Arc<Mutex<Value>>,
    /// How a refresh is answered: `rotate`, `keep`, `5xx` or `invalid`.
    refresh: Arc<Mutex<&'static str>>,
}

impl Knobs {
    fn refresh(&self) -> &'static str {
        *self.refresh.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn meta(&self) -> Value {
        self.meta
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Fake {
    fn start(registration: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen: Seen = Arc::default();
        let knobs = Knobs::default();
        *knobs.refresh.lock().unwrap() = "rotate";
        let (b, s, k) = (base.clone(), Arc::clone(&seen), knobs.clone());
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { return };
                let (method, path, body) = read_request(&mut conn);
                let (status, headers, reply) = route(&b, (registration, &k), &method, &path, &body);
                s.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push((method, path, body));
                let _ = write!(
                    conn,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
            }
        });
        Self { base, seen, knobs }
    }

    fn mcp(&self) -> String {
        format!("{}/mcp", self.base)
    }

    fn hits(&self, method: &str, path: &str) -> Vec<String> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(m, p, _)| m == method && p == path)
            .map(|(_, _, b)| b.clone())
            .collect()
    }
}

fn read_request(conn: &mut std::net::TcpStream) -> (String, String, String) {
    let mut r = BufReader::new(conn);
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    let mut parts = line.split_whitespace();
    let (method, path) = (
        parts.next().unwrap().to_owned(),
        parts.next().unwrap().to_owned(),
    );
    let mut len = 0;
    loop {
        let mut h = String::new();
        r.read_line(&mut h).unwrap();
        if h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                len = v.trim().parse().unwrap();
            }
        }
    }
    let mut body = vec![0; len];
    r.read_exact(&mut body).unwrap();
    (method, path, String::from_utf8(body).unwrap())
}

fn route(
    base: &str,
    (registration, knobs): (bool, &Knobs),
    method: &str,
    path: &str,
    body: &str,
) -> (&'static str, String, String) {
    let ok = |v: Value| ("200 OK", String::new(), v.to_string());
    match (method, path) {
        ("POST", "/mcp") => (
            "401 Unauthorized",
            format!("WWW-Authenticate: Bearer resource_metadata=\"{base}/.well-known/oauth-protected-resource/mcp\"\r\n"),
            String::new(),
        ),
        ("GET", "/.well-known/oauth-protected-resource/mcp") => ok(json!({
            "resource": format!("{base}/mcp"),
            "authorization_servers": [base],
            "scopes_supported": ["read"],
        })),
        ("GET", "/.well-known/oauth-authorization-server") => {
            let mut meta = json!({
                "issuer": base,
                "authorization_endpoint": format!("{base}/authorize"),
                "token_endpoint": format!("{base}/token"),
                "code_challenge_methods_supported": ["S256"],
                "authorization_response_iss_parameter_supported": true,
            });
            if registration {
                meta["registration_endpoint"] = json!(format!("{base}/register"));
            }
            if let Value::Object(patch) = knobs.meta() {
                for (k, v) in patch {
                    meta[k] = v;
                }
            }
            ok(meta)
        }
        ("POST", "/register") => ("201 Created", String::new(), json!({"client_id": "dcr-client"}).to_string()),
        ("POST", "/token") if body.contains("code=good") && body.contains("code_verifier=") => ok(json!({
            "access_token": "at-1",
            "token_type": "Bearer",
            "expires_in": 3600,
            "refresh_token": "rt-1",
        })),
        ("POST", "/token") if body.contains("grant_type=refresh_token") => match knobs.refresh() {
            "rotate" => ok(json!({"access_token": "at-2", "token_type": "Bearer", "expires_in": 3600, "refresh_token": "rt-2"})),
            "keep" => ok(json!({"access_token": "at-3", "token_type": "Bearer", "expires_in": 3600})),
            "5xx" => ("503 Service Unavailable", String::new(), "{}".to_owned()),
            _ => ("400 Bad Request", String::new(), json!({"error": "invalid_grant"}).to_string()),
        },
        ("POST", "/token") => ("400 Bad Request", String::new(), json!({"error": "invalid_grant"}).to_string()),
        _ => ("404 Not Found", String::new(), "{}".to_owned()),
    }
}

/// A fixture on a clock the test moves, with `config` in place.
fn setup(config: UserConfig) -> (Fixture, Arc<FakeClock>) {
    let f = fixture();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_800_000_000_000)));
    let cx =
        f.cx.with_clock(Arc::clone(&clock) as Arc<dyn Clock>)
            .with_config(config);
    (Fixture { cx, ..f }, clock)
}

fn user(f: &Fixture, who: &str) -> UserId {
    f.cx.store().unwrap().enroll(&id(who)).unwrap().0
}

fn add(f: &Fixture, who: &str, url: &str) -> Result<Value, crate::error::CommandError> {
    let cmd = UserCmd::McpAdd {
        name: "notion".into(),
        url: url.into(),
        header: None,
        key: None,
        machine: false,
    };
    run(&f.cx, &f.vault, &id(who), &cmd)
}

fn finish(
    f: &Fixture,
    who: &str,
    state: &str,
    code: &str,
    iss: Option<&str>,
) -> Result<Value, crate::error::CommandError> {
    let cmd = UserCmd::McpOAuthFinish {
        state: state.into(),
        code: code.into(),
        iss: iss.map(str::to_owned),
    };
    run(&f.cx, &f.vault, &id(who), &cmd)
}

fn query(url: &str) -> BTreeMap<String, String> {
    Url::parse(url)
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}

/// Adds the fake's server as `who` and returns the authorize address's query.
fn signed_in_start(f: &Fixture, fake: &Fake, who: &str) -> BTreeMap<String, String> {
    user(f, who);
    let value = add(f, who, &fake.mcp()).unwrap();
    query(value["sign_in"].as_str().unwrap())
}

fn secret(f: &Fixture, vault: &Vault, user: UserId) -> Option<String> {
    let name = SecretName::parse("mcp_notion").unwrap();
    f.cx.store()
        .unwrap()
        .user_secret(vault, user, &name)
        .unwrap()
        .map(|s| String::from_utf8(s.expose().to_vec()).unwrap())
}

#[test]
fn a_401_naming_resource_metadata_leads_to_an_authorize_address_with_resource_and_s256() {
    let fake = Fake::start(true);
    let (f, _) = setup(UserConfig::default());
    let q = signed_in_start(&f, &fake, "local:2000");
    assert_eq!(q["response_type"], "code");
    assert_eq!(q["client_id"], "dcr-client");
    assert_eq!(q["code_challenge_method"], "S256");
    assert_eq!(q["code_challenge"].len(), 43, "{q:?}");
    assert_eq!(q["resource"], fake.mcp());
    assert_eq!(q["scope"], "read");
    assert_eq!(q["redirect_uri"], "http://127.0.0.1:33418/oauth/callback");
    assert!(!q["state"].is_empty());
    let reg: Value = serde_json::from_str(&fake.hits("POST", "/register")[0]).unwrap();
    assert_eq!(
        reg["redirect_uris"],
        json!(["http://127.0.0.1:33418/oauth/callback"])
    );
}

#[test]
fn finishing_stores_the_access_token_as_the_proxy_secret_and_the_grant_apart() {
    let fake = Fake::start(true);
    let (f, _) = setup(UserConfig::default());
    let q = signed_in_start(&f, &fake, "local:2000");
    let reply = finish(&f, "local:2000", &q["state"], "good", Some(&fake.base)).unwrap();
    assert_eq!(reply["name"], "notion");
    let me = user(&f, "local:2000");
    assert_eq!(secret(&f, &f.vault, me).as_deref(), Some("at-1"));
    let token = &fake.hits("POST", "/token")[0];
    let form: BTreeMap<String, String> = Url::parse(&format!("http://x/?{token}"))
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect();
    assert_eq!(form["resource"], fake.mcp());
    assert_eq!(form["code"], "good");
    assert!(!form["code_verifier"].is_empty());
    let list = run(&f.cx, &f.vault, &id("local:2000"), &UserCmd::McpList).unwrap();
    assert_eq!(
        list["servers"][0]["signed_in"],
        json!(1_800_000_000_000_i64 + 3_600_000)
    );
    assert_eq!(list["servers"][0]["has_key"], true);
}

#[test]
fn remove_drops_the_access_token_and_the_grant() {
    let fake = Fake::start(true);
    let (f, _) = setup(UserConfig::default());
    let q = signed_in_start(&f, &fake, "local:2000");
    finish(&f, "local:2000", &q["state"], "good", Some(&fake.base)).unwrap();
    let me = user(&f, "local:2000");
    let store = f.cx.store().unwrap();
    assert_eq!(store.user_mcp_oauth_state(me).unwrap().len(), 1);
    run(
        &f.cx,
        &f.vault,
        &id("local:2000"),
        &UserCmd::McpRemove {
            name: "notion".into(),
        },
    )
    .unwrap();
    assert_eq!(
        secret(&f, &f.vault, me),
        None,
        "remove drops the access token"
    );
    assert!(
        store.user_mcp_oauth_state(me).unwrap().is_empty(),
        "remove drops the grant"
    );
}

fn refused(r: Result<Value, crate::error::CommandError>) -> ErrorKind {
    r.unwrap_err().kind
}

#[test]
fn another_user_cannot_finish_my_sign_in() {
    let fake = Fake::start(true);
    let (f, _) = setup(UserConfig::default());
    let q = signed_in_start(&f, &fake, "local:2000");
    user(&f, "local:3000");
    assert_eq!(
        refused(finish(
            &f,
            "local:3000",
            &q["state"],
            "good",
            Some(&fake.base)
        )),
        ErrorKind::Usage
    );
    assert!(fake.hits("POST", "/token").is_empty());
    finish(&f, "local:2000", &q["state"], "good", Some(&fake.base)).unwrap();
}

#[test]
fn an_expired_sign_in_is_refused() {
    let fake = Fake::start(true);
    let (f, clock) = setup(UserConfig::default());
    let q = signed_in_start(&f, &fake, "local:2000");
    clock.sleep(Duration::from_secs(601));
    assert_eq!(
        refused(finish(
            &f,
            "local:2000",
            &q["state"],
            "good",
            Some(&fake.base)
        )),
        ErrorKind::Usage
    );
    assert!(fake.hits("POST", "/token").is_empty());
}

#[test]
fn a_wrong_or_missing_issuer_is_refused() {
    let fake = Fake::start(true);
    let (f, _) = setup(UserConfig::default());
    let q = signed_in_start(&f, &fake, "local:2000");
    assert_eq!(
        refused(finish(
            &f,
            "local:2000",
            &q["state"],
            "good",
            Some("https://evil.example")
        )),
        ErrorKind::Usage
    );
    let q = query(
        add(&f, "local:2000", &fake.mcp()).unwrap()["sign_in"]
            .as_str()
            .unwrap(),
    );
    assert_eq!(
        refused(finish(&f, "local:2000", &q["state"], "good", None)),
        ErrorKind::Usage
    );
    assert!(fake.hits("POST", "/token").is_empty());
}

#[test]
fn a_state_finishes_once() {
    let fake = Fake::start(true);
    let (f, _) = setup(UserConfig::default());
    let q = signed_in_start(&f, &fake, "local:2000");
    finish(&f, "local:2000", &q["state"], "good", Some(&fake.base)).unwrap();
    assert_eq!(
        refused(finish(
            &f,
            "local:2000",
            &q["state"],
            "good",
            Some(&fake.base)
        )),
        ErrorKind::Usage
    );
    assert_eq!(fake.hits("POST", "/token").len(), 1);
}

#[test]
fn an_operator_client_skips_registration() {
    let fake = Fake::start(false);
    let mut config = UserConfig::default();
    config.oauth.clients.insert(
        "127.0.0.1".into(),
        OAuthClient {
            client_id: "pre".into(),
            client_secret: Some("shh".into()),
        },
    );
    let (f, _) = setup(config);
    let q = signed_in_start(&f, &fake, "local:2000");
    assert_eq!(q["client_id"], "pre");
    assert!(fake.hits("POST", "/register").is_empty());
    finish(&f, "local:2000", &q["state"], "good", Some(&fake.base)).unwrap();
    assert_eq!(fake.hits("POST", "/token").len(), 1);
}

#[test]
fn a_server_that_does_not_answer_401_is_added_as_before() {
    let (f, _) = setup(UserConfig::default());
    user(&f, "local:2000");
    let value = add(&f, "local:2000", "http://127.0.0.1:9/mcp").unwrap();
    assert_eq!(value.get("sign_in"), None);
    let list = run(&f.cx, &f.vault, &id("local:2000"), &UserCmd::McpList).unwrap();
    assert_eq!(list["servers"][0]["has_key"], false);
}

#[test]
fn an_issuer_that_is_not_the_discovered_one_is_refused() {
    let fake = Fake::start(true);
    *fake.knobs.meta.lock().unwrap() = json!({"issuer": "https://elsewhere.example"});
    let (f, _) = setup(UserConfig::default());
    user(&f, "local:2000");
    let err = add(&f, "local:2000", &fake.mcp()).unwrap_err();
    assert!(
        err.error
            .to_string()
            .contains("names issuer https://elsewhere.example"),
        "{err:?}"
    );
}

#[test]
fn a_plain_http_token_endpoint_is_refused() {
    let fake = Fake::start(true);
    *fake.knobs.meta.lock().unwrap() = json!({"token_endpoint": "http://tokens.example/token"});
    let (f, _) = setup(UserConfig::default());
    user(&f, "local:2000");
    let err = add(&f, "local:2000", &fake.mcp()).unwrap_err();
    assert!(
        err.error
            .to_string()
            .contains("http://tokens.example/token is not https"),
        "{err:?}"
    );
}

mod refresh;
