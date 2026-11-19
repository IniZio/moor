//! Run now: fire a workflow once, as the session's person.

use std::net::TcpStream;

use moor_core::request::{Request as ControllerRequest, WorkflowRunNowReq};
use moor_core::response::ExitClass;
use moor_core::view::workflow::RunNowValue;
use serde_json::json;

use super::{decode, not_found};
use crate::login::account::{failed, json};
use crate::login::repos::answered;
use crate::login::{check_csrf, Session};
use crate::{tools, Gate};

/// Fires the workflow `name`, when the request carries the CSRF token. The controller keeps it
/// to the workflow's owner. A run it refuses, such as while another is open, answers `409` with
/// the reason.
pub(crate) fn run_now(
    stream: &mut TcpStream,
    gate: &Gate,
    session: &Session,
    csrf: Option<&str>,
    name: &str,
) {
    if !csrf.is_some_and(|c| check_csrf(session, c)) {
        let error = json!({"error": "write from the page you are signed in on"});
        return json(stream, "403 Forbidden", &error);
    }
    let request = ControllerRequest::WorkflowRunNow(WorkflowRunNowReq {
        identity: None,
        name: decode(name),
        expected_revision: None,
    });
    let reply = match tools::ask_as(&gate.controller, &session.person, request) {
        Ok(reply) => reply,
        Err(why) => return failed(stream, "run now", &why),
    };
    if reply.class == ExitClass::Refused {
        // Every workflow the controller shows a person is theirs, so a refusal is no such workflow.
        return not_found(stream);
    }
    if reply.class != ExitClass::Ok {
        return answered(stream, &reply);
    }
    let value = reply.value.clone().unwrap_or_default();
    let Ok(line) = serde_json::from_value::<RunNowValue>(value) else {
        return failed(stream, "run now", "unreadable answer");
    };
    if line.status == "refused" {
        let why = line.reason.unwrap_or_default();
        return json(stream, "409 Conflict", &json!({"error": why}));
    }
    json(
        stream,
        "200 OK",
        &json!({"status": line.status, "box": line.box_name}),
    );
}
