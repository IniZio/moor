//! Browser sign-in at `/auth/*` against the testkit issuer, whose token endpoint refuses a
//! code unless the PKCE verifier matches the challenge the authorize request carried.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

use moor_gate::{Gate, Login, Verifier};
use moor_testkit::oidc::Issuer;

use super::CLIENT_ID;

const PAGE_HEADERS: [&str; 4] = [
    "Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'\r\n",
    "X-Content-Type-Options: nosniff\r\n",
    "Referrer-Policy: no-referrer\r\n",
    "Cache-Control: no-store\r\n",
];

pub(super) struct Reply {
    pub(super) status: u16,
    head: String,
    pub(super) body: String,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.head.lines().find_map(|line| {
            let (n, v) = line.split_once(": ")?;
            n.eq_ignore_ascii_case(name).then_some(v)
        })
    }

    pub(super) fn location(&self) -> &str {
        self.header("location").unwrap()
    }
}

/// A gate with sign-in on, served at its own loopback address.
fn gate() -> (String, Issuer) {
    let issuer = Issuer::start(CLIENT_ID);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let login = Login::new(&format!("http://{addr}")).unwrap();
    let gate = Arc::new(Gate {
        front_socket: "/nonexistent.sock".into(),
        mcp: None,
        verifier: Verifier::new(issuer.base.clone(), CLIENT_ID.to_owned(), None),
    });
    std::thread::spawn(move || moor_gate::serve_with(&listener, &gate, Some(&Arc::new(login))));
    (addr, issuer)
}

pub(super) fn send(addr: &str, method: &str, target: &str, extra: &str, body: &str) -> Reply {
    let mut stream = TcpStream::connect(addr).unwrap();
    write!(
        stream,
        "{method} {target} HTTP/1.1\r\nHost: {addr}\r\n{extra}Content-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut out = String::new();
    stream.read_to_string(&mut out).unwrap();
    let (head, body) = out.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    Reply {
        status,
        head: format!("{head}\r\n"),
        body: body.to_owned(),
    }
}

pub(super) fn get(addr: &str, target: &str, cookie: Option<&str>) -> Reply {
    let extra = cookie.map_or_else(String::new, |c| format!("Cookie: {c}\r\n"));
    send(addr, "GET", target, &extra, "")
}

/// `http://host/path?query` as its host and path-with-query.
fn split(url: &str) -> (&str, &str) {
    let rest = url.strip_prefix("http://").unwrap();
    rest.split_at(rest.find('/').unwrap())
}

fn query(url: &str) -> HashMap<String, String> {
    let q = url.split_once('?').unwrap().1;
    oauth2::url::form_urlencoded::parse(q.as_bytes())
        .into_owned()
        .collect()
}

/// The issuer's redirect back to the gate for the sign-in `authorize` starts.
fn approve(authorize: &str) -> String {
    let (host, target) = split(authorize);
    let back = get(host, target, None);
    assert_eq!(back.status, 302);
    split(back.location()).1.to_owned()
}

/// Signs in, landing on `/auth/me`: the session cookie pair.
pub(super) fn sign_in(addr: &str) -> String {
    let start = get(addr, "/auth/login?return=/auth/me", None);
    let landed = get(addr, &approve(start.location()), None);
    assert_eq!(landed.status, 302, "{}", landed.body);
    let cookie = landed.header("set-cookie").unwrap();
    cookie.split(';').next().unwrap().to_owned()
}

pub(super) fn assert_page_headers(reply: &Reply) {
    for h in PAGE_HEADERS {
        assert!(reply.head.contains(h), "{h} missing from {}", reply.head);
    }
}

pub(super) fn csrf(page: &Reply) -> String {
    let (_, rest) = page.body.split_once("name=\"csrf\" value=\"").unwrap();
    rest.split('"').next().unwrap().to_owned()
}

#[test]
fn login_redirects_to_the_issuer_with_pkce_and_a_fresh_state() {
    let (addr, issuer) = gate();
    let first = get(&addr, "/auth/login?return=/auth/me", None);
    assert_eq!(first.status, 302, "{}", first.body);
    assert_page_headers(&first);
    let url = first.location();
    assert!(
        url.starts_with(&format!("{}/authorize?", issuer.base)),
        "{url}"
    );
    let q = query(url);
    assert_eq!(q["response_type"], "code");
    assert_eq!(q["client_id"], CLIENT_ID);
    assert_eq!(q["redirect_uri"], format!("http://{addr}/auth/callback"));
    assert_eq!(q["scope"], "openid email");
    assert_eq!(q["code_challenge_method"], "S256");
    assert!(q["state"].len() >= 43, "{}", q["state"]);
    let second = get(&addr, "/auth/login?return=/auth/me", None);
    assert_ne!(query(second.location())["state"], q["state"]);
}

#[test]
fn login_refuses_a_return_that_leaves_the_site() {
    let (addr, _) = gate();
    for bad in [
        "",
        "?return=",
        "?return=evil",
        "?return=%2F%2Fevil.com",
        "?return=//evil.com",
        "?return=https://evil.com",
        "?return=/%5Cevil.com",
        "?return=/a%0D%0ASet-Cookie:x=1",
        "?return=/a%0Ab",
        "?return=/a%20b",
    ] {
        let reply = get(&addr, &format!("/auth/login{bad}"), None);
        assert_eq!(reply.status, 400, "{bad}");
        assert!(reply.header("location").is_none(), "{bad}");
    }
}

#[test]
fn a_finished_sign_in_sets_a_strict_session_cookie_and_returns() {
    let (addr, _) = gate();
    let start = get(&addr, "/auth/login?return=/auth/me%3Ftab%3D1", None);
    let landed = get(&addr, &approve(start.location()), None);
    assert_eq!(landed.status, 302, "{}", landed.body);
    assert_eq!(landed.location(), "/auth/me?tab=1");
    let cookie = landed.header("set-cookie").unwrap();
    let (pair, attrs) = cookie.split_once("; ").unwrap();
    let id = pair.strip_prefix("moor_session=").unwrap();
    assert!(id.len() >= 43, "{id}");
    assert_eq!(attrs, "HttpOnly; Secure; SameSite=Lax; Path=/");
}

#[test]
fn a_state_works_once() {
    let (addr, _) = gate();
    let start = get(&addr, "/auth/login?return=/auth/me", None);
    let callback = approve(start.location());
    assert_eq!(get(&addr, &callback, None).status, 302);
    let again = get(&addr, &callback, None);
    assert_eq!(again.status, 400);
    assert!(again.header("set-cookie").is_none());
    assert_eq!(
        get(&addr, "/auth/callback?code=x&state=unknown", None).status,
        400
    );
}

#[test]
fn a_code_trades_only_with_its_own_verifier() {
    let (addr, _) = gate();
    let a = get(&addr, "/auth/login?return=/auth/me", None);
    let b = get(&addr, "/auth/login?return=/auth/me", None);
    let code = query(&approve(a.location()))["code"].clone();
    let state = query(b.location())["state"].clone();
    let crossed = get(
        &addr,
        &format!("/auth/callback?code={code}&state={state}"),
        None,
    );
    assert_eq!(crossed.status, 502);
    assert!(crossed.header("set-cookie").is_none());
}

#[test]
fn me_names_the_signed_in_person_and_sends_others_to_sign_in() {
    let (addr, _) = gate();
    let away = get(&addr, "/auth/me", None);
    assert_eq!(away.status, 302);
    assert_eq!(away.location(), "/auth/login?return=/auth/me");
    let forged = get(&addr, "/auth/me", Some("moor_session=forged"));
    assert_eq!(forged.status, 302);
    let cookie = sign_in(&addr);
    let me = get(&addr, "/auth/me", Some(&format!("theme=dark; {cookie}")));
    assert_eq!(me.status, 200);
    assert!(
        me.body.contains("Signed in as alice@example.com"),
        "{}",
        me.body
    );
    assert_page_headers(&me);
}

#[test]
fn logout_needs_the_session_csrf_token() {
    let (addr, _) = gate();
    let cookie = sign_in(&addr);
    let token = csrf(&get(&addr, "/auth/me", Some(&cookie)));
    let with = |body: &str| {
        let extra =
            format!("Cookie: {cookie}\r\nContent-Type: application/x-www-form-urlencoded\r\n");
        send(&addr, "POST", "/auth/logout", &extra, body)
    };
    assert_eq!(with("").status, 403);
    assert_eq!(with("csrf=wrong").status, 403);
    let other = sign_in(&addr);
    let other_token = csrf(&get(&addr, "/auth/me", Some(&other)));
    assert_eq!(with(&format!("csrf={other_token}")).status, 403);
    assert_eq!(get(&addr, "/auth/me", Some(&cookie)).status, 200);
    let out = with(&format!("csrf={token}"));
    assert_eq!(out.status, 200);
    assert!(out
        .header("set-cookie")
        .unwrap()
        .starts_with("moor_session=;"));
    assert_eq!(get(&addr, "/auth/me", Some(&cookie)).status, 302);
    assert_eq!(get(&addr, "/auth/me", Some(&other)).status, 200);
}

#[test]
fn auth_is_not_found_without_a_public_url() {
    let w = super::world();
    for path in ["/auth/me", "/auth/login?return=/auth/me", "/auth/callback"] {
        assert_eq!(get(&w.gate, path, None).status, 404, "{path}");
    }
}
