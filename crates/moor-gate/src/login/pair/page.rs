//! The confirm page at `/pair`: a signed-in person sees what a code would add and adds it.

use std::collections::HashMap;
use std::net::TcpStream;
use std::time::SystemTime;

use moor_core::request::PairOp;
use moor_core::view::user::MachinePairValue;
use oauth2::url::form_urlencoded;
use serde_json::Value;

use super::super::routes::{escape, page, read_form, redirect, Request};
use super::super::{check_csrf, csrf_token, Login, Session};
use super::{enroll_request, normalize, step, Ask, PATH};
use crate::{log, tools, Gate};

/// Serves the confirm page: `GET` shows what a code would add, `POST` adds it.
pub(crate) fn handle(mut stream: TcpStream, req: Request<'_>, gate: &Gate, login: &Login) {
    #[allow(
        clippy::disallowed_methods,
        reason = "session expiry; the store takes the time as an argument"
    )]
    let now = SystemTime::now();
    let session = login.session(req.cookies.iter().copied(), now);
    let query = req.target.split_once('?').map_or("", |(_, q)| q);
    let code = form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == "code")
        .map_or_else(String::new, |(_, v)| normalize(&v));
    match (req.method, session) {
        ("GET", None) => {
            let back = format!("{PATH}?code={code}");
            let back: String = form_urlencoded::byte_serialize(back.as_bytes()).collect();
            redirect(&mut stream, &format!("/auth/login?return={back}"), "");
        }
        ("GET", Some(session)) => match waiting(gate, PairOp::Show { code: code.clone() }) {
            Ok(Some(ask)) => {
                let body = format!(
                    "Add the machine <strong>{name}</strong> to your machines, as {email}? It will run your Boxes.</p><p>Check the machine shows the code <code>{code}</code>.</p><form method=\"post\" action=\"{PATH}\"><input type=\"hidden\" name=\"csrf\" value=\"{csrf}\"><input type=\"hidden\" name=\"code\" value=\"{code}\"><button>Add {name}</button></form><p>",
                    name = escape(&ask.name),
                    email = escape(session.person.email.as_str()),
                    code = escape(&code),
                    csrf = escape(csrf_token(&session)),
                );
                page(&mut stream, "200 OK", "", &body);
            }
            Ok(None) => gone(&mut stream),
            Err(why) => failed(&mut stream, &why),
        },
        ("POST", session) => {
            let form = read_form(&mut stream, req.leftover, req.length);
            confirm(&mut stream, gate, session, form.as_ref());
        }
        _ => page(&mut stream, "404 Not Found", "", "Not found."),
    }
}

/// What the code `op` names asks for, when `op` finds it waiting.
fn waiting(gate: &Gate, op: PairOp) -> Result<Option<Ask>, String> {
    match step(&gate.controller, op)? {
        MachinePairValue::Waiting { ask } => serde_json::from_value(ask)
            .map(Some)
            .map_err(|e| format!("moor kept a pairing the gate cannot read: {e}")),
        _ => Ok(None),
    }
}

/// Enrolls the posted code's machine as the session's person, when the form carries the
/// session's CSRF token. The controller lets one confirm claim a code; a failed enroll puts it
/// back to waiting.
fn confirm(
    stream: &mut TcpStream,
    gate: &Gate,
    session: Option<Session>,
    form: Option<&HashMap<String, String>>,
) {
    let csrf = form.and_then(|f| f.get("csrf"));
    let Some(session) = session.filter(|s| csrf.is_some_and(|c| check_csrf(s, c))) else {
        return page(
            stream,
            "403 Forbidden",
            "",
            "Add a machine from the page you are signed in on.",
        );
    };
    let code = normalize(form.and_then(|f| f.get("code")).map_or("", |c| c));
    let ask = match waiting(gate, PairOp::Claim { code: code.clone() }) {
        Ok(Some(ask)) => ask,
        Ok(None) => return gone(stream),
        Err(why) => return failed(stream, &why),
    };
    let enrolled: Result<Value, String> =
        tools::act_as(&gate.controller, &session.person, enroll_request(&ask));
    let settle = PairOp::Settle {
        code,
        enrolled: enrolled.as_ref().ok().cloned(),
    };
    match (enrolled, step(&gate.controller, settle)) {
        (Ok(_), Ok(_)) => page(
            stream,
            "200 OK",
            "",
            &format!(
                "Added <strong>{}</strong>. Go back to the machine; it finishes on its own.",
                escape(&ask.name)
            ),
        ),
        (Err(why), _) | (Ok(_), Err(why)) => failed(stream, &why),
    }
}

/// A `502` for a pairing that failed on the way to or in the controller.
fn failed(stream: &mut TcpStream, why: &str) {
    log(&format!("pairing a machine failed: {why}"));
    page(stream, "502 Bad Gateway", "", &escape(why));
}

/// A `404` for a code that is not waiting.
fn gone(stream: &mut TcpStream) {
    page(
        stream,
        "404 Not Found",
        "",
        "This code is unknown, used or expired. Run <code>moor-machine-agent up</code> on the machine again for a new one.",
    );
}
