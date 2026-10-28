#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::missing_docs_in_private_items,
    clippy::too_many_lines,
    reason = "tests assert on fixed fixtures"
)]

//! Signing in to a fake OIDC server, and refreshing what that saved.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use moor_client::TokenSource;
use moor_herdr::auth::login::{self, Ui};
use moor_herdr::auth::{Credentials, Store, Tokens};
use oauth2::url::Url;
use sha2::{Digest, Sha256};

const NOW: i64 = 1_000_000;

#[derive(Default)]
struct State {
    /// Challenge each issued code was authorized with.
    codes: HashMap<String, String>,
    /// Current refresh token.
    refresh: String,
    refreshes: usize,
}

/// A tiny identity provider and moor server in one: discovery, authorize, token.
struct Fake {
    base: String,
    state: Arc<Mutex<State>>,
}

fn respond(stream: &mut TcpStream, status: &str, extra: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\n{extra}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn handle(mut stream: TcpStream, base: &str, state: &Mutex<State>) {
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).unwrap_or(0) == 0 {
            return;
        }
        head.push(byte[0]);
    }
    let head = String::from_utf8(head).unwrap();
    let mut words = head.lines().next().unwrap().split(' ');
    let (method, target) = (words.next().unwrap(), words.next().unwrap());
    let len = head
        .lines()
        .find_map(|l| {
            l.to_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse().unwrap())
        })
        .unwrap_or(0_usize);
    let mut body = vec![0; len];
    stream.read_exact(&mut body).unwrap();
    let url = Url::parse(&format!("{base}{target}")).unwrap();
    let query: HashMap<String, String> = url.query_pairs().into_owned().collect();
    match (method, url.path()) {
        ("GET", "/.well-known/moor") => respond(
            &mut stream,
            "200 OK",
            "",
            &format!(
                r#"{{"issuer":"{base}","client_id":"moor-cli","scopes":["openid","email","offline_access"]}}"#
            ),
        ),
        ("GET", "/.well-known/openid-configuration") => respond(
            &mut stream,
            "200 OK",
            "",
            &format!(
                r#"{{"issuer":"{base}","authorization_endpoint":"{base}/authorize","token_endpoint":"{base}/token","code_challenge_methods_supported":["S256"]}}"#
            ),
        ),
        ("GET", "/authorize") => {
            let code = format!("code-{}", query["state"].len());
            state
                .lock()
                .unwrap()
                .codes
                .insert(code.clone(), query["code_challenge"].clone());
            let back = format!(
                "{}?code={code}&state={}",
                query["redirect_uri"], query["state"]
            );
            respond(
                &mut stream,
                "302 Found",
                &format!("Location: {back}\r\n"),
                "",
            );
        }
        ("POST", "/token") => token(&mut stream, &String::from_utf8(body).unwrap(), state),
        _ => respond(&mut stream, "404 Not Found", "", "{}"),
    }
}

fn token(stream: &mut TcpStream, body: &str, state: &Mutex<State>) {
    let form: HashMap<String, String> = oauth2::url::form_urlencoded::parse(body.as_bytes())
        .into_owned()
        .collect();
    let refuse = |stream: &mut TcpStream, why: &str| {
        respond(
            stream,
            "400 Bad Request",
            "",
            &format!(r#"{{"error":"invalid_grant","error_description":"{why}"}}"#),
        );
    };
    let mut st = state.lock().unwrap();
    match form["grant_type"].as_str() {
        "authorization_code" => {
            let Some(challenge) = st.codes.remove(&form["code"]) else {
                return refuse(stream, "unknown code");
            };
            let verifier = form.get("code_verifier").map_or("", String::as_str);
            if URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) != challenge {
                return refuse(stream, "pkce verifier does not match");
            }
            "refresh-0".clone_into(&mut st.refresh);
            respond(
                stream,
                "200 OK",
                "",
                r#"{"access_token":"access-0","token_type":"Bearer","expires_in":3600,"refresh_token":"refresh-0"}"#,
            );
        }
        "refresh_token" if form["refresh_token"] == st.refresh => {
            st.refreshes += 1;
            let n = st.refreshes;
            st.refresh = format!("refresh-{n}");
            drop(st);
            std::thread::sleep(Duration::from_millis(200));
            respond(
                stream,
                "200 OK",
                "",
                &format!(
                    r#"{{"access_token":"access-{n}","token_type":"Bearer","expires_in":3600,"refresh_token":"refresh-{n}"}}"#
                ),
            );
        }
        _ => refuse(stream, "bad grant"),
    }
}

fn fake() -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let state = Arc::new(Mutex::new(State::default()));
    let (b, s) = (base.clone(), Arc::clone(&state));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (b, s) = (b.clone(), Arc::clone(&s));
            std::thread::spawn(move || handle(stream, &b, &s));
        }
    });
    Fake { base, state }
}

/// What the person does: the browser opens the sign-in address, and the return either
/// reaches the loopback or is pasted.
struct Person {
    paste: Option<Sender<String>>,
    via_loopback: bool,
    first_paste: Vec<String>,
}

impl Ui for Person {
    fn say(&mut self, _line: &str) {}

    fn open(&mut self, url: &str) {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .max_redirects(0)
            .http_status_as_error(false)
            .build()
            .into();
        let resp = agent.get(url).call().unwrap();
        let back = resp
            .headers()
            .get("location")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        if self.via_loopback {
            agent.get(&back).call().unwrap();
        } else {
            let tx = self.paste.take().unwrap();
            for junk in self.first_paste.drain(..) {
                tx.send(junk).unwrap();
            }
            tx.send(back).unwrap();
        }
    }

    fn pasted(&mut self, to: Sender<String>) {
        self.paste = Some(to);
    }
}

const fn person(via_loopback: bool) -> Person {
    Person {
        paste: None,
        via_loopback,
        first_paste: Vec::new(),
    }
}

fn sign_in(fake: &Fake, store: &Store, mut who: Person) -> Result<(), String> {
    login::run(&fake.base, 0, store, &mut who, NOW)
}

fn saved(fake: &Fake, expires_at: i64) -> Credentials {
    Credentials {
        server: fake.base.clone(),
        issuer: fake.base.clone(),
        client_id: "moor-cli".to_owned(),
        token_endpoint: format!("{}/token", fake.base),
        refresh_token: "refresh-0".to_owned(),
        access_token: "access-0".to_owned(),
        expires_at,
    }
}

#[test]
fn pasting_the_callback_completes_login_and_saves_0600_credentials() {
    let fake = fake();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path());
    let mut who = person(false);
    who.first_paste = vec![
        "not a url".to_owned(),
        "http://localhost:1/callback?code=x&state=other".to_owned(),
    ];
    sign_in(&fake, &store, who).unwrap();
    let creds = store.read().unwrap().unwrap();
    assert_eq!(creds.server, fake.base);
    assert_eq!(creds.access_token, "access-0");
    assert_eq!(creds.refresh_token, "refresh-0");
    assert_eq!(creds.expires_at, NOW + 3600);
    let mode = std::fs::metadata(dir.path().join("credentials.json"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn the_browser_returning_to_the_loopback_completes_login() {
    let fake = fake();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path());
    sign_in(&fake, &store, person(true)).unwrap();
    assert_eq!(store.read().unwrap().unwrap().access_token, "access-0");
}

#[test]
fn an_unreachable_server_saves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path());
    let err = login::run("http://127.0.0.1:1", 0, &store, &mut person(false), NOW).unwrap_err();
    assert!(err.contains("cannot reach"), "{err}");
    assert!(store.read().unwrap().is_none());
}

#[test]
fn a_token_with_time_left_is_used_as_it_is() {
    let fake = fake();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path());
    store.write(&saved(&fake, NOW + 3600)).unwrap();
    let tokens = Tokens::with_clock(store, &format!("{}/", fake.base), || NOW);
    assert_eq!(tokens.bearer().unwrap(), "access-0");
    assert_eq!(fake.state.lock().unwrap().refreshes, 0);
}

#[test]
fn two_processes_refreshing_together_refresh_once() {
    let fake = fake();
    fake.state.lock().unwrap().refresh = "refresh-0".to_owned();
    let dir = tempfile::tempdir().unwrap();
    Store::new(dir.path())
        .write(&saved(&fake, NOW + 60))
        .unwrap();
    let spawn = || {
        let (path, base) = (dir.path().to_owned(), fake.base.clone());
        std::thread::spawn(move || {
            Tokens::with_clock(Store::new(path), &base, || NOW)
                .bearer()
                .unwrap()
        })
    };
    let (one, two) = (spawn(), spawn());
    let got = [one.join().unwrap(), two.join().unwrap()];
    assert_eq!(got, ["access-1", "access-1"]);
    assert_eq!(fake.state.lock().unwrap().refreshes, 1);
    let creds = Store::new(dir.path()).read().unwrap().unwrap();
    assert_eq!(
        (creds.refresh_token.as_str(), creds.expires_at),
        ("refresh-1", NOW + 3600)
    );
    let mode = std::fs::metadata(dir.path().join("credentials.json"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn credentials_for_another_server_or_none_say_to_log_in() {
    let fake = fake();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path());
    let none = Tokens::with_clock(store.clone(), &fake.base, || NOW)
        .bearer()
        .unwrap_err();
    assert!(none.contains("moor-herdr login"), "{none}");
    store.write(&saved(&fake, NOW + 3600)).unwrap();
    let other = Tokens::with_clock(store.clone(), "https://example.com", || NOW)
        .bearer()
        .unwrap_err();
    assert!(
        other.contains("signed in to") && other.contains("example.com"),
        "{other}"
    );
    assert!(store.delete().unwrap());
    assert!(!store.delete().unwrap());
}

#[test]
fn a_revoked_refresh_token_says_the_sign_in_ended() {
    let fake = fake();
    fake.state.lock().unwrap().refresh = "refresh-other".to_owned();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path());
    store.write(&saved(&fake, NOW + 60)).unwrap();
    let err = Tokens::with_clock(store, &fake.base, || NOW)
        .bearer()
        .unwrap_err();
    assert!(err.contains("ended") && err.contains("login"), "{err}");
}
