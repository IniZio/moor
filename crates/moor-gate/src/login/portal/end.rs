//! Ending a Box's work from the portal: Stop parks it asleep, Close removes it.

use std::net::TcpStream;

use moor_core::name::BoxName;
use moor_core::request::{BoxReq, CloseReq, Request as ControllerRequest};
use moor_core::response::{ErrorDetail, ExitClass, Response as ControllerResponse};
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

/// A refused close with its kind and counts, so the page words it without the controller's commands.
fn close_refusal(reply: &ControllerResponse) -> Option<serde_json::Value> {
    let error = reply
        .error
        .as_ref()
        .filter(|_| reply.class != ExitClass::Ok)?;
    let detail = error.detail.as_ref()?;
    matches!(detail, ErrorDetail::CloseRefused(_))
        .then(|| json!({"error": error.message, "refusal": detail}))
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
        Ok(reply) => match close_refusal(&reply) {
            Some(refusal) => json(stream, "403 Forbidden", &refusal),
            None => answered(stream, &reply),
        },
        Err(why) => failed(stream, "box end", &why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use moor_core::response::CloseRefusal;

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
    fn a_refused_close_carries_its_kind_and_counts_beside_the_text() {
        let refused = |detail: Option<ErrorDetail>| ControllerResponse {
            schema: moor_core::SCHEMA,
            cmd: "close".to_owned(),
            class: ExitClass::Refused,
            value: None,
            warnings: Vec::new(),
            error: Some(moor_core::response::ErrorView {
                kind: moor_core::response::ErrorKind::Refused,
                message: "run `moor close --force`".to_owned(),
                hints: Vec::new(),
                context: Vec::new(),
                detail,
            }),
        };
        let unsaved = CloseRefusal::Unsaved {
            branch: "b".to_owned(),
            ahead: 2,
            dirty: 1,
        };
        let got = close_refusal(&refused(Some(ErrorDetail::CloseRefused(unsaved))));
        let want = json!({
            "error": "run `moor close --force`",
            "refusal": {"kind": "close_refused", "refusal": "unsaved", "branch": "b", "ahead": 2, "dirty": 1},
        });
        assert_eq!(got, Some(want));
        assert!(close_refusal(&refused(Some(ErrorDetail::AgentGone))).is_none());
        assert!(close_refusal(&refused(None)).is_none());
    }

    #[test]
    fn an_ending_must_name_a_box() {
        for bad in [r#"{"box": "../x"}"#, "{}", "not json"] {
            assert!(request(How::Close, Some(bad.as_bytes())).is_none(), "{bad}");
        }
        assert!(request(How::Stop, None).is_none());
    }
}
