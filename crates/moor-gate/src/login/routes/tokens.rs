//! `/tokens`: a signed-in person lists, creates and revokes their API tokens.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::net::TcpStream;

use moor_core::request::{ApiTokenCreateReq, ApiTokenRevokeReq, AsReq, Request};
use moor_core::view::user::{
    ApiTokenCreateValue, ApiTokenListValue, ApiTokenRevokeValue, ApiTokenView,
};

use super::{check_csrf, csrf_token, escape, page, redirect, Session};
use crate::api_token::{ask_as, Failure};
use crate::mcp::{READ, WRITE};
use crate::{log, Gate};

/// Serves one `/tokens` request; `form` is the posted body.
pub(super) fn handle(
    stream: &mut TcpStream,
    method: &str,
    path: &str,
    session: Option<Session>,
    form: Option<&HashMap<String, String>>,
    gate: &Gate,
) {
    let Some(session) = session else {
        if method == "GET" {
            return redirect(stream, "/auth/login?return=/tokens", "");
        }
        return forbidden(stream);
    };
    match (method, path) {
        ("GET", "/tokens") => return list(stream, &session, gate, ""),
        ("POST", _) => {}
        _ => return page(stream, "404 Not Found", "", "Not found."),
    }
    let posted = form.filter(|f| f.get("csrf").is_some_and(|c| check_csrf(&session, c)));
    let Some(form) = posted else {
        return forbidden(stream);
    };
    if path == "/tokens" {
        return create(stream, &session, form, gate);
    }
    let id = path
        .strip_prefix("/tokens/")
        .and_then(|rest| rest.strip_suffix("/revoke"))
        .and_then(|id| id.parse().ok());
    match id {
        Some(id) => revoke(stream, &session, id, gate),
        None => page(stream, "404 Not Found", "", "Not found."),
    }
}

/// Mints a token from the form and shows its text, once.
fn create(stream: &mut TcpStream, session: &Session, form: &HashMap<String, String>, gate: &Gate) {
    let scopes = [("read", READ), ("write", WRITE)]
        .into_iter()
        .filter(|(field, _)| form.contains_key(*field))
        .map(|(_, scope)| scope.to_owned())
        .collect();
    let request = Request::ApiTokenCreate(ApiTokenCreateReq {
        identity: None,
        name: form.get("name").map_or("", |n| n.trim()).to_owned(),
        scopes,
    });
    match ask_as::<ApiTokenCreateValue>(&gate.front_socket, &session.person, request) {
        Ok(created) => {
            let text = String::from_utf8_lossy(created.token.secret().expose()).into_owned();
            let body = format!(
                "Your new token:</p><p><code>{}</code></p><p>Copy it now; it won't be shown again.</p><p><a href=\"/tokens\">Back to your tokens</a>",
                escape(&text)
            );
            page(stream, "200 OK", "", &body);
        }
        Err(Failure::Refused(why)) => list(stream, session, gate, &why),
        Err(Failure::Unavailable(why)) => unavailable(stream, &why),
    }
}

/// Revokes token `id`, then goes back to the list.
fn revoke(stream: &mut TcpStream, session: &Session, id: i64, gate: &Gate) {
    let request = Request::ApiTokenRevoke(ApiTokenRevokeReq { identity: None, id });
    match ask_as::<ApiTokenRevokeValue>(&gate.front_socket, &session.person, request) {
        Ok(_) => page(
            stream,
            "303 See Other",
            "Location: /tokens\r\n",
            "Redirecting.",
        ),
        Err(Failure::Refused(why)) => list(stream, session, gate, &why),
        Err(Failure::Unavailable(why)) => unavailable(stream, &why),
    }
}

/// The person's tokens and the form for a new one, with `message` above them when set.
fn list(stream: &mut TcpStream, session: &Session, gate: &Gate, message: &str) {
    let request = Request::ApiTokenList(AsReq { identity: None });
    let listed = match ask_as::<ApiTokenListValue>(&gate.front_socket, &session.person, request) {
        Ok(listed) => listed,
        Err(Failure::Refused(why) | Failure::Unavailable(why)) => return unavailable(stream, &why),
    };
    let csrf = escape(csrf_token(session));
    let mut rows = String::new();
    for token in &listed.tokens {
        row(&mut rows, token, &csrf);
    }
    if rows.is_empty() {
        rows.push_str("<tr><td colspan=\"6\">No tokens yet.</td></tr>");
    }
    let alert = if message.is_empty() {
        String::new()
    } else {
        format!("<p role=\"alert\">{}</p>", escape(message))
    };
    let body = format!(
        "Your API tokens for {who}.</p>{alert}<table><tr><th>Name</th><th>Scopes</th><th>Ends in</th><th>Created</th><th>Last used</th><th></th></tr>{rows}</table><form method=\"post\" action=\"/tokens\"><input type=\"hidden\" name=\"csrf\" value=\"{csrf}\"><label>Name <input name=\"name\" required></label> <label><input type=\"checkbox\" name=\"read\" checked> {READ}</label> <label><input type=\"checkbox\" name=\"write\"> {WRITE}</label> <button>Create token</button></form><p>",
        who = escape(session.person.email.as_str()),
    );
    page(stream, "200 OK", "", &body);
}

/// Appends `token`'s table row, with its revoke form.
fn row(rows: &mut String, token: &ApiTokenView, csrf: &str) {
    let used = token.last_used_at.map_or_else(|| "never".to_owned(), date);
    let _ = write!(
        rows,
        "<tr><td>{}</td><td>{}</td><td>\u{2026}{}</td><td>{}</td><td>{}</td><td><form method=\"post\" action=\"/tokens/{}/revoke\"><input type=\"hidden\" name=\"csrf\" value=\"{csrf}\"><button>Revoke</button></form></td></tr>",
        escape(&token.name),
        escape(&token.scopes.join(" ")),
        escape(&token.hint),
        date(token.created_at),
        used,
        token.id,
    );
}

/// Unix milliseconds as `YYYY-MM-DD HH:MM UTC`, by the civil-from-days algorithm.
fn date(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let (hour, minute) = (rest / 3600, rest % 3600 / 60);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02} UTC")
}

/// The refusal of a post without a session or its CSRF token.
fn forbidden(stream: &mut TcpStream) {
    page(
        stream,
        "403 Forbidden",
        "",
        "Post from the tokens page you are signed in on.",
    );
}

/// Logs why the controller gave no answer and says so.
fn unavailable(stream: &mut TcpStream, why: &str) {
    log(&format!("/tokens: {why}"));
    page(
        stream,
        "502 Bad Gateway",
        "",
        "The controller is unreachable. Try again.",
    );
}
