//! Ending a Box's work from the portal: Stop parks it asleep, Close removes it.

use std::net::TcpStream;

use moor_core::name::BoxName;
use moor_core::request::{BoxReq, CloseReq, Request as ControllerRequest};
use serde::Deserialize;
use serde_json::json;

use super::super::account::{failed, json};
use super::super::repos::answered;
use super::super::{check_csrf, Session};
use crate::{tools, Gate};

/// Which Box, and whether to close it past work that is not pushed.
#[derive(Deserialize)]
struct End {
    /// The Box's name, as the list shows it.
    #[serde(rename = "box")]
    boxed: String,
    /// Discard unpushed work; only a person who saw the controller's refusal sends it.
    #[serde(default)]
    force: bool,
}

/// What an ending does to the Box.
#[derive(Clone, Copy)]
pub(super) enum How {
    /// Park the machine; a later message wakes it.
    Stop,
    /// Remove the Box.
    Close,
}

/// The request `how` is for the Box a body names.
fn request(how: How, body: Option<&[u8]>) -> Option<ControllerRequest> {
    let end: End = serde_json::from_slice(body?).ok()?;
    let name = BoxName::parse_allow_bare(&end.boxed).ok()?;
    Some(match how {
        How::Stop => ControllerRequest::Sleep(BoxReq { box_name: name }),
        How::Close => ControllerRequest::Close(CloseReq {
            target: name.into(),
            force: end.force,
        }),
    })
}

/// Runs the ending as the session's person, through the request `sleep` or `close_box` sends.
/// A refusal, such as a Box holding unpushed work, is for the person to read.
pub(super) fn end(
    stream: &mut TcpStream,
    gate: &Gate,
    session: &Session,
    csrf: Option<&str>,
    body: Option<&[u8]>,
    how: How,
) {
    if !csrf.is_some_and(|c| check_csrf(session, c)) {
        let error = json!({"error": "write from the page you are signed in on"});
        return json(stream, "403 Forbidden", &error);
    }
    let Some(asked) = request(how, body) else {
        return json(stream, "400 Bad Request", &json!({"error": "name a Box"}));
    };
    match tools::ask_as(&gate.controller, &session.person, asked) {
        Ok(reply) => answered(stream, &reply),
        Err(why) => failed(stream, "box end", &why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_parks_and_close_removes_and_only_a_close_can_be_forced() {
        let body = Some(br#"{"box": "acme/web-1", "force": true}"#.as_slice());
        assert!(matches!(
            request(How::Stop, body),
            Some(ControllerRequest::Sleep(_))
        ));
        let forced = request(How::Close, body);
        assert!(matches!(forced, Some(ControllerRequest::Close(c)) if c.force));
        let plain = request(How::Close, Some(br#"{"box": "web-1"}"#.as_slice()));
        assert!(matches!(plain, Some(ControllerRequest::Close(c)) if !c.force));
    }

    #[test]
    fn an_ending_must_name_a_box() {
        for bad in [r#"{"box": "../x"}"#, "{}", "not json"] {
            assert!(request(How::Close, Some(bad.as_bytes())).is_none(), "{bad}");
        }
        assert!(request(How::Stop, None).is_none());
    }
}
