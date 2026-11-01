//! The `/auth/*` routes and the headers every page they serve carries.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Instant;

use oauth2::url::form_urlencoded;

use super::{check_csrf, csrf_token, Login, Session};
use crate::{log, Gate};

/// The session cookie's name.
pub(crate) const COOKIE: &str = "moor_session";

/// Headers on every page: nothing loads but inline styles, forms post only here, nothing
/// frames it, and nothing caches it or leaks it in a referrer.
const PAGE_HEADERS: &str = "Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nCache-Control: no-store\r\n";

/// Largest form body the routes read.
const FORM_MAX: usize = 4096;

/// What one `/auth/*` request carries beside its stream.
pub(crate) struct Request<'a> {
    /// The method.
    pub(crate) method: &'a str,
    /// The path with its query.
    pub(crate) target: &'a str,
    /// Every `Cookie` header value.
    pub(crate) cookies: Vec<&'a str>,
    /// The declared body length.
    pub(crate) length: Option<usize>,
    /// Body bytes read with the head.
    pub(crate) leftover: Vec<u8>,
}

/// Serves one `/auth/*` request.
pub(crate) fn handle(mut stream: TcpStream, req: Request<'_>, gate: &Gate, login: &Login) {
    #[allow(
        clippy::disallowed_methods,
        reason = "sign-in and session expiry; the store takes the time as an argument"
    )]
    let now = Instant::now();
    let (path, query) = req.target.split_once('?').unwrap_or((req.target, ""));
    let query: HashMap<String, String> = form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();
    let session = login.session(req.cookies.iter().copied(), now);
    match (req.method, path) {
        ("GET", "/auth/login") => {
            let Some(back) = query.get("return").filter(|p| same_origin(p)) else {
                return page(
                    &mut stream,
                    "400 Bad Request",
                    "",
                    "Sign-in needs a return path on this site.",
                );
            };
            match login.start(&gate.verifier, back, now) {
                Ok(url) => redirect(&mut stream, &url, ""),
                Err(why) => failed(&mut stream, &why),
            }
        }
        ("GET", "/auth/callback") => callback(&mut stream, &query, gate, login, now),
        ("GET", "/auth/me") => match session {
            Some(session) => me(&mut stream, &session),
            None => redirect(&mut stream, "/auth/login?return=/auth/me", ""),
        },
        ("POST", "/auth/logout") => {
            let form = read_form(&mut stream, req.leftover, req.length);
            logout(&mut stream, login, session, form.as_ref());
        }
        _ => page(&mut stream, "404 Not Found", "", "Not found."),
    }
}

/// Finishes the sign-in the `state` names, setting the session cookie.
fn callback(
    stream: &mut TcpStream,
    query: &HashMap<String, String>,
    gate: &Gate,
    login: &Login,
    now: Instant,
) {
    let pending = query.get("state").and_then(|s| login.take(s, now));
    let (Some(pending), Some(code)) = (pending, query.get("code")) else {
        return page(
            stream,
            "400 Bad Request",
            "",
            "This sign-in link is expired or was already used. Sign in again.",
        );
    };
    match login.finish(&gate.verifier, pending, code, now) {
        Ok((session, back)) => {
            let cookie = format!(
                "Set-Cookie: {COOKIE}={}; HttpOnly; Secure; SameSite=Lax; Path=/\r\n",
                session.id
            );
            redirect(stream, &back, &cookie);
        }
        Err(why) => failed(stream, &why),
    }
}

/// Ends `session` when the form carries its CSRF token.
fn logout(
    stream: &mut TcpStream,
    login: &Login,
    session: Option<Session>,
    form: Option<&HashMap<String, String>>,
) {
    match (session, form.and_then(|f| f.get("csrf"))) {
        (Some(session), Some(csrf)) if check_csrf(&session, csrf) => {
            login.close(&session);
            let clear = format!(
                "Set-Cookie: {COOKIE}=; Max-Age=0; HttpOnly; Secure; SameSite=Lax; Path=/\r\n"
            );
            page(stream, "200 OK", &clear, "Signed out.");
        }
        _ => page(
            stream,
            "403 Forbidden",
            "",
            "Sign out from the page you are signed in on.",
        ),
    }
}

/// Whether `path` stays on this site: one leading slash, then printable ASCII with no
/// backslash, so no browser reads it as another host.
fn same_origin(path: &str) -> bool {
    path.starts_with('/')
        && !path.starts_with("//")
        && path.bytes().all(|b| b.is_ascii_graphic() && b != b'\\')
}

/// The signed-in page, with a sign-out form.
fn me(stream: &mut TcpStream, session: &Session) {
    let body = format!(
        "Signed in as {}.</p><form method=\"post\" action=\"/auth/logout\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><button>Sign out</button></form><p>",
        escape(session.person.email.as_str()),
        escape(csrf_token(session)),
    );
    page(stream, "200 OK", "", &body);
}

/// Logs why a sign-in failed and tells the browser to try again.
fn failed(stream: &mut TcpStream, why: &str) {
    log(&format!("sign-in failed: {why}"));
    page(stream, "502 Bad Gateway", "", "Sign-in failed. Try again.");
}

/// A `302` to `location`, with `extra` headers.
pub(super) fn redirect(stream: &mut TcpStream, location: &str, extra: &str) {
    page(
        stream,
        "302 Found",
        &format!("Location: {location}\r\n{extra}"),
        "Redirecting.",
    );
}

/// Writes one HTML page around the already-escaped `body`, with [`PAGE_HEADERS`] and `extra`.
pub(super) fn page(stream: &mut TcpStream, status: &str, extra: &str, body: &str) {
    let html = format!("<!doctype html><meta charset=\"utf-8\"><title>moor</title><p>{body}</p>\n");
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\n{PAGE_HEADERS}{extra}Content-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
}

/// `text` safe inside HTML text and quoted attributes.
pub(super) fn escape(text: &str) -> String {
    text.chars().fold(String::new(), |mut out, c| {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
        out
    })
}

/// The form-encoded body, when it fits and arrives whole.
pub(super) fn read_form(
    stream: &mut TcpStream,
    mut body: Vec<u8>,
    length: Option<usize>,
) -> Option<HashMap<String, String>> {
    let length = length.filter(|n| *n <= FORM_MAX)?;
    body.truncate(length);
    let owed = u64::try_from(length.saturating_sub(body.len())).ok()?;
    stream.take(owed).read_to_end(&mut body).ok()?;
    (body.len() == length).then(|| form_urlencoded::parse(&body).into_owned().collect())
}
