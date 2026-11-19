//! A Box's changes in git for the portal's Changes tab, and the wake that tab offers when the
//! Box is asleep and only its last receipt can be shown.

use std::net::TcpStream;

use moor_core::name::BoxName;
use moor_core::request::{BoxReq, ChangesReq, Request as ControllerRequest};
use moor_core::response::ExitClass;
use serde::Deserialize;
use serde_json::json;

use super::super::account::{failed, json};
use super::super::repos::answered;
use super::super::{check_csrf, Session};
use super::param;
use crate::{tools, Gate};

/// Answers the changes of the Box the query names, as the session's person.
pub(super) fn changes(stream: &mut TcpStream, gate: &Gate, session: &Session, query: &str) {
    let Some(name) = param(query, "box").and_then(|b| BoxName::parse_allow_bare(&b).ok()) else {
        return json(stream, "400 Bad Request", &json!({"error": "name a Box"}));
    };
    let request = ControllerRequest::Changes(ChangesReq {
        target: name.into(),
    });
    match tools::ask_as(&gate.controller, &session.person, request) {
        // Every Box the controller shows a person is theirs, so a refusal is no such Box.
        Ok(reply) if !matches!(reply.class, ExitClass::Ok | ExitClass::Usage) => {
            json(stream, "404 Not Found", &json!({"error": "no such Box"}));
        }
        Ok(reply) => answered(stream, &reply),
        Err(why) => failed(stream, "box changes", &why),
    }
}

/// Which Box to wake.
#[derive(Deserialize)]
struct Wake {
    /// The Box's name, as the list shows it.
    #[serde(rename = "box")]
    boxed: String,
}

/// The Box a body names.
fn target(body: Option<&[u8]>) -> Option<BoxName> {
    let wake: Wake = serde_json::from_slice(body?).ok()?;
    BoxName::parse_allow_bare(&wake.boxed).ok()
}

/// Wakes the session's person's Box, as `wake_box` does.
pub(super) fn wake(
    stream: &mut TcpStream,
    gate: &Gate,
    session: &Session,
    csrf: Option<&str>,
    body: Option<&[u8]>,
) {
    if !csrf.is_some_and(|c| check_csrf(session, c)) {
        let error = json!({"error": "write from the page you are signed in on"});
        return json(stream, "403 Forbidden", &error);
    }
    let Some(box_name) = target(body) else {
        return json(stream, "400 Bad Request", &json!({"error": "name a Box"}));
    };
    let request = ControllerRequest::Wake(BoxReq { box_name });
    match tools::ask_as(&gate.controller, &session.person, request) {
        Ok(reply) => answered(stream, &reply),
        Err(why) => failed(stream, "box wake", &why),
    }
}

#[cfg(test)]
mod tests {
    use super::target;

    #[test]
    fn a_wake_must_name_a_box() {
        assert!(target(Some(br#"{"box": "acme/web-1"}"#)).is_some());
        for bad in [r#"{"box": "../x"}"#, "{}", "not json"] {
            assert!(target(Some(bad.as_bytes())).is_none(), "{bad}");
        }
    }
}
