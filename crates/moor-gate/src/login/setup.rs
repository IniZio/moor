//! The webhook setup page at `/webhooks/<id>/setup`: a signed-in person sees a new webhook's
//! signing key and gives the value of the header its receiver wants, so neither secret
//! passes through an agent.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::net::TcpStream;
use std::time::SystemTime;

use moor_core::request::{
    Request as ControllerRequest, SecretValue, WebhookSetupFinishReq, WebhookSetupShowReq,
};
use moor_core::view::user::{WebhookSetupValue, WebhookState};

use super::routes::{escape, page, read_form, redirect, Request};
use super::{check_csrf, csrf_token, Login, Session};
use crate::{log, tools, Gate};

/// The controller's refusal for a webhook that is not the person's.
const NO_SUCH: &str = "no such webhook";

/// Serves one `/webhooks/*` request.
pub(crate) fn handle(mut stream: TcpStream, req: Request<'_>, gate: &Gate, login: &Login) {
    #[allow(
        clippy::disallowed_methods,
        reason = "session expiry; the store takes the time as an argument"
    )]
    let now = SystemTime::now();
    let path = req.target.split_once('?').map_or(req.target, |(p, _)| p);
    let Some(id) = setup_id(path) else {
        return not_found(&mut stream);
    };
    let session = login.session(req.cookies.iter().copied(), now);
    match (req.method, session) {
        ("GET", Some(session)) => match setup(gate, &session, id) {
            Ok(value) => render(&mut stream, "200 OK", &session, &value, None),
            Err(why) => refused(&mut stream, &why),
        },
        ("GET", None) => redirect(
            &mut stream,
            &format!("/auth/login?return=/webhooks/{id}/setup"),
            "",
        ),
        ("POST", session) => {
            let form = read_form(&mut stream, req.leftover, req.length);
            finish(&mut stream, gate, session, form.as_ref(), id);
        }
        _ => not_found(&mut stream),
    }
}

/// The webhook id in `/webhooks/<id>/setup`, when it is a positive integer.
fn setup_id(path: &str) -> Option<i64> {
    let id = path.strip_prefix("/webhooks/")?.strip_suffix("/setup")?;
    if !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    id.parse().ok().filter(|id| *id > 0)
}

/// Webhook `id` as its setup page shows it, asked for as the session's person.
fn setup(gate: &Gate, session: &Session, id: i64) -> Result<WebhookSetupValue, String> {
    let show = ControllerRequest::WebhookSetupShow(WebhookSetupShowReq { identity: None, id });
    let value = tools::act_as(&gate.controller, &session.person, show)?;
    serde_json::from_value(value).map_err(|e| format!("the controller's answer is unreadable: {e}"))
}

/// Finishes setup when the form carries the session's CSRF token, passing the header value
/// on without ever writing it back.
fn finish(
    stream: &mut TcpStream,
    gate: &Gate,
    session: Option<Session>,
    form: Option<&HashMap<String, String>>,
    id: i64,
) {
    let (Some(session), Some(form)) = (session, form) else {
        return forbidden(stream);
    };
    if !form.get("csrf").is_some_and(|c| check_csrf(&session, c)) {
        return forbidden(stream);
    }
    let header_value = form
        .get("header_value")
        .filter(|v| !v.is_empty())
        .map(|v| SecretValue::new(v.clone().into_bytes()));
    let request = ControllerRequest::WebhookSetupFinish(WebhookSetupFinishReq {
        identity: None,
        id,
        header_value,
    });
    match tools::act_as(&gate.controller, &session.person, request) {
        Ok(_) => page(
            stream,
            "200 OK",
            "",
            "Set up. Deliveries start with the next event.",
        ),
        Err(why) if why == NO_SUCH => not_found(stream),
        Err(why) => match setup(gate, &session, id) {
            Ok(value) => render(stream, "400 Bad Request", &session, &value, Some(&why)),
            Err(why) => refused(stream, &why),
        },
    }
}

/// The setup page for `value`, with `notice` above it: the key and the form while it waits.
fn render(
    stream: &mut TcpStream,
    status: &str,
    session: &Session,
    value: &WebhookSetupValue,
    notice: Option<&str>,
) {
    let mut body = format!(
        "Webhook {} posts to <code>{}</code>.",
        value.id,
        escape(&value.url)
    );
    if let Some(notice) = notice {
        let _ = write!(body, "</p><p><strong>{}</strong>", escape(notice));
    }
    if value.state == WebhookState::Active {
        body.push_str("</p><p>It is set up; the signing key is no longer shown.");
        return page(stream, status, "", &body);
    }
    if let Some(key) = &value.key {
        let key = String::from_utf8_lossy(key.secret().expose());
        let _ = write!(body,
            "</p><p>Signing key: <code>{}</code></p><p>Save this key now; it is shown only while setup is unfinished. The receiver checks X-Moor-Signature with it.",
            escape(&key)
        );
    }
    let _ = write!(body,
        "</p><form method=\"post\" action=\"/webhooks/{}/setup\"><input type=\"hidden\" name=\"csrf\" value=\"{}\">",
        value.id,
        escape(csrf_token(session))
    );
    if let Some(header) = &value.header {
        let _ = write!(body,
            "<p><label>{} value <input type=\"password\" name=\"header_value\" autocomplete=\"off\" placeholder=\"Bearer …\" required></label></p>",
            escape(header)
        );
    }
    body.push_str("<button>Finish setup</button></form><p>");
    page(stream, status, "", &body);
}

/// Answers a refusal: not found for someone else's webhook, otherwise the controller's words.
fn refused(stream: &mut TcpStream, why: &str) {
    if why == NO_SUCH {
        return not_found(stream);
    }
    log(&format!("webhook setup failed: {why}"));
    page(stream, "502 Bad Gateway", "", &escape(why));
}

/// A `404`.
fn not_found(stream: &mut TcpStream) {
    page(stream, "404 Not Found", "", "Not found.");
}

/// A `403` for a post without the session or its CSRF token.
fn forbidden(stream: &mut TcpStream) {
    page(
        stream,
        "403 Forbidden",
        "",
        "Finish setup from the page you are signed in on.",
    );
}
