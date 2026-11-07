//! The secrets page at `/me/secrets`: a signed-in person stores the tokens their Boxes use, so
//! none passes through an agent or needs a shell on the controller's host.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::net::TcpStream;
use std::time::Instant;

use moor_core::request::{
    AsReq, Request as ControllerRequest, SecretSetReq, SecretValue, StartNeedsReq,
};
use moor_core::user::SecretName;
use moor_core::view::user::{SecretNamesValue, StartNeedsValue};

use super::routes::{escape, page, read_form, redirect, Request};
use super::{check_csrf, csrf_token, Login, Session};
use crate::{log, tools, Gate};

/// The page's path.
pub(crate) const PATH: &str = "/me/secrets";

/// The secrets every Box on GitHub needs, each with where its token comes from, as HTML.
const SOURCES: [(&str, &str); 2] = [
    (
        "claude",
        "Paste what <code>claude setup-token</code> prints on your own machine.",
    ),
    (
        "github",
        "Paste what <code>gh auth token</code> prints, or a fine-grained token that can push to the repos you work on.",
    ),
];

/// One secret as the page lists it.
struct Row {
    /// The secret's name.
    name: String,
    /// Whether the person has stored it.
    set: bool,
}

/// Serves one `/me/secrets` request.
pub(crate) fn handle(mut stream: TcpStream, req: Request<'_>, gate: &Gate, login: &Login) {
    #[allow(
        clippy::disallowed_methods,
        reason = "session expiry; the store takes the time as an argument"
    )]
    let now = Instant::now();
    let session = login.session(req.cookies.iter().copied(), now);
    match (req.method, session) {
        ("GET", Some(session)) => show(&mut stream, gate, &session, "200 OK", None),
        ("GET", None) => redirect(&mut stream, &format!("/auth/login?return={PATH}"), ""),
        ("POST", session) => {
            let form = read_form(&mut stream, req.leftover, req.length);
            save(&mut stream, gate, session, form.as_ref());
        }
        _ => page(&mut stream, "404 Not Found", "", "Not found."),
    }
}

/// Stores the posted secret as the session's person when the form carries its CSRF token.
/// The value goes only to the controller: never into a log or back to the browser.
fn save(
    stream: &mut TcpStream,
    gate: &Gate,
    session: Option<Session>,
    form: Option<&HashMap<String, String>>,
) {
    let (Some(session), Some(form)) = (session, form) else {
        return forbidden(stream);
    };
    if !form.get("csrf").is_some_and(|c| check_csrf(&session, c)) {
        return forbidden(stream);
    }
    let name = form.get("name").and_then(|n| SecretName::parse(n).ok());
    let value = form.get("value").filter(|v| !v.is_empty());
    let (Some(name), Some(value)) = (name, value) else {
        let notice = "Choose a secret and paste its value.";
        return show(stream, gate, &session, "400 Bad Request", Some(notice));
    };
    let set = ControllerRequest::SecretSet(SecretSetReq {
        identity: None,
        name: name.clone(),
        value: SecretValue::new(value.clone().into_bytes()),
    });
    match tools::act_as(&gate.controller, &session.person, set) {
        Ok(_) => {
            let notice = format!("Saved <code>{}</code>.", escape(name.as_str()));
            show(stream, gate, &session, "200 OK", Some(&notice));
        }
        Err(why) => {
            log(&format!("storing secret {} failed: {why}", name.as_str()));
            show(
                stream,
                gate,
                &session,
                "502 Bad Gateway",
                Some(&escape(&why)),
            );
        }
    }
}

/// The secrets the person's Boxes need and whether each is set, asked as the session's person.
fn rows(gate: &Gate, session: &Session) -> Result<Vec<Row>, String> {
    let ask = |request| tools::act_as(&gate.controller, &session.person, request);
    let unreadable = |e: serde_json::Error| format!("the controller's answer is unreadable: {e}");
    let needs = ask(ControllerRequest::StartNeeds(StartNeedsReq {
        identity: None,
        repo: None,
    }))?;
    let needs: StartNeedsValue = serde_json::from_value(needs).map_err(unreadable)?;
    let have = ask(ControllerRequest::SecretNames(AsReq { identity: None }))?;
    let have: SecretNamesValue = serde_json::from_value(have).map_err(unreadable)?;
    let mut names: Vec<String> = SOURCES.iter().map(|(n, _)| (*n).to_owned()).collect();
    for name in needs.needs {
        if !names.iter().any(|n| n == name.as_str()) {
            names.push(name.as_str().to_owned());
        }
    }
    Ok(names
        .into_iter()
        .map(|name| Row {
            set: have.names.iter().any(|h| h.as_str() == name),
            name,
        })
        .collect())
}

/// The page with `notice`, already HTML, above the list; the controller's words when it fails.
fn show(
    stream: &mut TcpStream,
    gate: &Gate,
    session: &Session,
    status: &str,
    notice: Option<&str>,
) {
    let rows = match rows(gate, session) {
        Ok(rows) => rows,
        Err(why) => {
            log(&format!("the secrets page failed: {why}"));
            return page(stream, "502 Bad Gateway", "", &escape(&why));
        }
    };
    let mut body = String::new();
    if let Some(notice) = notice {
        let _ = write!(body, "<strong>{notice}</strong></p><p>");
    }
    body.push_str("Your Boxes use these secrets. Values are never shown; saving one replaces it.");
    let csrf = escape(csrf_token(session));
    for row in rows {
        let name = escape(&row.name);
        let state = if row.set { "set" } else { "not set" };
        let _ = write!(body, "</p><h2><code>{name}</code>: {state}</h2><p>");
        if let Some((_, source)) = SOURCES.iter().find(|(n, _)| *n == row.name) {
            body.push_str(source);
        }
        let _ = write!(body,
            "</p><form method=\"post\" action=\"{PATH}\"><input type=\"hidden\" name=\"csrf\" value=\"{csrf}\"><input type=\"hidden\" name=\"name\" value=\"{name}\"><p><label>{name} <input type=\"password\" name=\"value\" autocomplete=\"off\" required></label> <button>Save</button></p></form><p>"
        );
    }
    page(stream, status, "", &body);
}

/// A `403` for a post without the session or its CSRF token.
fn forbidden(stream: &mut TcpStream) {
    page(
        stream,
        "403 Forbidden",
        "",
        "Save secrets from the page you are signed in on.",
    );
}
