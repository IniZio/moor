//! The account page: the built app at the gate root, and the JSON it reads and writes under
//! `/account/api`. It links secrets and shows machines; it never starts a Box.

use std::io::Write as _;
use std::net::TcpStream;
use std::time::SystemTime;

use moor_core::request::{AsReq, Request as ControllerRequest, SecretSetReq, SecretValue};
use moor_core::secret::{check_claude_token, check_github_token, Secret};
use moor_core::user::SecretName;
use moor_core::view::node::NodeListValue;
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::routes::{page, read_body, redirect, Request};
use super::secrets::{rows, Row};
use super::{check_csrf, csrf_token, Login, Session};
use crate::{log, tools, Gate};

/// The app's built files: `index.html` and `assets/`. Rebuilt by `make web`.
#[derive(RustEmbed)]
#[folder = "web/dist"]
struct Built;

/// Where the built files are served from.
const ASSETS: &str = "/account/assets/";
/// The JSON the app reads.
const API: &str = "/account/api";
/// Where a secret is written: this, then its name.
const SECRETS: &str = "/account/api/secrets/";

/// The largest secret write the page reads.
const BODY_MAX: usize = 8192;

/// Headers on the app page: its own script and styles, requests to itself, nothing framing it
/// and nothing cached.
const APP_HEADERS: &str = "Content-Security-Policy: default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nCache-Control: no-store\r\n";

/// Whether `path` belongs to the account page.
pub(crate) fn serves(path: &str) -> bool {
    path == "/" || path.starts_with("/account/")
}

/// What `GET /account/api` answers: everything the page shows, for the signed-in person.
#[derive(Serialize)]
struct View<'a> {
    /// Who is signed in.
    email: &'a str,
    /// The token a write must send in `X-Moor-Csrf`.
    csrf: &'a str,
    /// The secrets Boxes need, linked or not; never a value.
    secrets: Vec<Row>,
    /// The nodes the person may use, as `list_nodes` shows them.
    machines: NodeListValue,
    /// How to connect from outside.
    connect: Connect<'a>,
}

/// The connect section's data.
#[derive(Serialize)]
struct Connect<'a> {
    /// The MCP endpoint to add to Claude Code, when this gate serves one.
    mcp_url: Option<&'a str>,
}

/// The body of a secret write.
#[derive(Deserialize)]
struct Write {
    /// The token to store.
    value: String,
}

/// Serves one request for the account page.
pub(crate) fn handle(mut stream: TcpStream, req: Request<'_>, gate: &Gate, login: &Login) {
    #[allow(
        clippy::disallowed_methods,
        reason = "session expiry; the store takes the time as an argument"
    )]
    let now = SystemTime::now();
    let session = login.session(req.cookies.iter().copied(), now);
    let path = req.target.split('?').next().unwrap_or_default();
    match (req.method, path, session) {
        ("GET", "/", Some(_)) => file(&mut stream, "index.html", APP_HEADERS),
        ("GET", "/", None) => redirect(&mut stream, "/auth/login?return=/", ""),
        ("GET", p, _) if p.starts_with(ASSETS) => {
            file(&mut stream, p.trim_start_matches("/account/"), "");
        }
        ("GET", API, Some(session)) => show(&mut stream, gate, &session),
        ("PUT", p, session) if p.starts_with(SECRETS) => {
            // Read the body before any answer: closing on unread bytes resets the connection
            // and the browser never sees the status.
            let body = read_body(&mut stream, req.leftover, req.length, BODY_MAX);
            let name = p.trim_start_matches(SECRETS);
            match session {
                Some(session) => link(&mut stream, gate, &session, req.csrf, name, body.as_deref()),
                None => signed_out(&mut stream),
            }
        }
        ("GET", p, None) if p.starts_with(API) => signed_out(&mut stream),
        _ => page(&mut stream, "404 Not Found", "", "Not found."),
    }
}

/// Answers `401`, which the app takes as the end of the session.
fn signed_out(stream: &mut TcpStream) {
    json(
        stream,
        "401 Unauthorized",
        &json!({"error": "sign in again"}),
    );
}

/// Answers with the embedded file `name`, or `404`.
fn file(stream: &mut TcpStream, name: &str, headers: &str) {
    let Some(found) = Built::get(name) else {
        return page(stream, "404 Not Found", "", "Not found.");
    };
    let kind = match name.rsplit('.').next() {
        Some("js") => "text/javascript",
        Some("css") => "text/css",
        _ => "text/html; charset=utf-8",
    };
    let cache = if headers.is_empty() {
        "Cache-Control: no-cache\r\n"
    } else {
        ""
    };
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\n{headers}{cache}X-Content-Type-Options: nosniff\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        found.data.len()
    );
    let _ = stream.write_all(&found.data);
}

/// Writes `value` as a JSON response that no cache keeps.
fn json(stream: &mut TcpStream, status: &str, value: &serde_json::Value) {
    let body = value.to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// Answers a failed controller call, logging why.
fn failed(stream: &mut TcpStream, what: &str, why: &str) {
    log(&format!("the account page's {what} failed: {why}"));
    json(stream, "502 Bad Gateway", &json!({"error": why}));
}

/// The page's data for the session's person, asked of the controller as them.
fn show(stream: &mut TcpStream, gate: &Gate, session: &Session) {
    let secrets = match rows(gate, session) {
        Ok(rows) => rows,
        Err(why) => return failed(stream, "secrets", &why),
    };
    let nodes = tools::act_as(
        &gate.controller,
        &session.person,
        ControllerRequest::NodeList(AsReq { identity: None }),
    )
    .and_then(|v| serde_json::from_value::<NodeListValue>(v).map_err(|e| e.to_string()));
    let machines = match nodes {
        Ok(machines) => machines,
        Err(why) => return failed(stream, "machines", &why),
    };
    let view = View {
        email: session.person.email.as_str(),
        csrf: csrf_token(session),
        secrets,
        machines,
        connect: Connect {
            mcp_url: gate.mcp.as_ref().map(crate::Mcp::resource_url),
        },
    };
    json(stream, "200 OK", &json!(view));
}

/// Why `value` cannot be secret `name`, found without asking anyone. Never quotes the value.
fn refusal(name: &str, value: &str) -> Option<String> {
    let token = Secret::new(value.as_bytes().to_vec());
    let checked = match name {
        "claude" => check_claude_token(&token),
        "github" => check_github_token(&token),
        _ if value.is_empty() => Err("is empty".to_owned()),
        _ => Ok(()),
    };
    checked.err().map(|why| format!("That value {why}."))
}

/// Stores the written secret as the session's person when the request carries the CSRF token.
/// The value goes only to the controller: never into a log or back to the browser.
fn link(
    stream: &mut TcpStream,
    gate: &Gate,
    session: &Session,
    csrf: Option<&str>,
    name: &str,
    body: Option<&[u8]>,
) {
    if !csrf.is_some_and(|c| check_csrf(session, c)) {
        let error = json!({"error": "write from the page you are signed in on"});
        return json(stream, "403 Forbidden", &error);
    }
    let write = body.and_then(|b| serde_json::from_slice::<Write>(b).ok());
    let (Ok(secret), Some(write)) = (SecretName::parse(name), write) else {
        let error = json!({"error": "name a secret and send its value"});
        return json(stream, "400 Bad Request", &error);
    };
    if let Some(why) = refusal(name, &write.value) {
        return json(stream, "400 Bad Request", &json!({"error": why}));
    }
    let set = ControllerRequest::SecretSet(SecretSetReq {
        identity: None,
        name: secret,
        value: SecretValue::new(write.value.into_bytes()),
    });
    match tools::act_as(&gate.controller, &session.person, set) {
        Ok(_) => json(stream, "200 OK", &json!({"name": name, "linked": true})),
        Err(why) => failed(stream, "secret write", &why),
    }
}
