//! The confirm page at `/pair`: a signed-in person sees what a code would add and adds it.

use std::collections::HashMap;
use std::net::TcpStream;
use std::time::SystemTime;

use moor_core::request::{MachineEnrollReq, Request as ControllerRequest};
use oauth2::url::form_urlencoded;

use super::super::routes::{escape, page, read_form, redirect, Request};
use super::super::{check_csrf, csrf_token, Login, Session};
use super::{normalize, Ask, PATH};
use crate::{log, tools, Gate};

/// Serves the confirm page: `GET` shows what a code would add, `POST` adds it.
pub(crate) fn handle(mut stream: TcpStream, req: Request<'_>, gate: &Gate, login: &Login) {
    #[allow(
        clippy::disallowed_methods,
        reason = "session and code expiry; the stores take the time as an argument"
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
        ("GET", Some(session)) => match login.pairings.waiting(&code, now) {
            Some(ask) => {
                let body = format!(
                    "Add the machine <strong>{name}</strong> to your machines, as {email}? It will run your Boxes.</p><p>Check the machine shows the code <code>{code}</code>.</p><form method=\"post\" action=\"{PATH}\"><input type=\"hidden\" name=\"csrf\" value=\"{csrf}\"><input type=\"hidden\" name=\"code\" value=\"{code}\"><button>Add {name}</button></form><p>",
                    name = escape(&ask.name),
                    email = escape(session.person.email.as_str()),
                    code = escape(&code),
                    csrf = escape(csrf_token(&session)),
                );
                page(&mut stream, "200 OK", "", &body);
            }
            None => gone(&mut stream),
        },
        ("POST", session) => {
            let form = read_form(&mut stream, req.leftover, req.length);
            confirm(&mut stream, gate, login, session, form.as_ref(), now);
        }
        _ => page(&mut stream, "404 Not Found", "", "Not found."),
    }
}

/// Enrolls the posted code's machine as the session's person, when the form carries the
/// session's CSRF token.
fn confirm(
    stream: &mut TcpStream,
    gate: &Gate,
    login: &Login,
    session: Option<Session>,
    form: Option<&HashMap<String, String>>,
    now: SystemTime,
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
    let enroll = |ask: &Ask| {
        let request = ControllerRequest::MachineEnroll(MachineEnrollReq {
            identity: None,
            email: None,
            name: ask.name.clone(),
            pubkey: ask.pubkey.clone(),
            labels: ask.labels.clone(),
            capacity: ask.slots,
            pool: ask.pool.clone(),
        });
        tools::act_as(&gate.controller, &session.person, request)
    };
    match login.pairings.confirm(&code, now, enroll) {
        None => gone(stream),
        Some(Ok(ask)) => page(
            stream,
            "200 OK",
            "",
            &format!(
                "Added <strong>{}</strong>. Go back to the machine; it finishes on its own.",
                escape(&ask.name)
            ),
        ),
        Some(Err(why)) => {
            log(&format!("pairing a machine failed: {why}"));
            page(stream, "502 Bad Gateway", "", &escape(&why));
        }
    }
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
