//! Run now: fire one repo of a workflow once, as the session's person.

use std::net::TcpStream;

use moor_core::request::{Request as ControllerRequest, WorkflowRunNowReq};
use moor_core::response::ExitClass;
use moor_core::view::workflow::RunNowValue;
use serde::Deserialize;
use serde_json::json;

use super::decode;
use crate::login::account::{failed, json};
use crate::login::repos::answered;
use crate::login::{check_csrf, Session};
use crate::{tools, Gate};

/// The body of a run now.
#[derive(Deserialize)]
struct NowBody {
    /// The repo, `owner/name`, to run the workflow for.
    repo: String,
}

/// Fires the workflow `name` for the repo in the body, when the request carries the CSRF token.
/// The controller keeps it to the workflow's owner. A repo it refuses, such as one with an open
/// run, answers `409` with the reason.
pub(crate) fn run_now(
    stream: &mut TcpStream,
    gate: &Gate,
    session: &Session,
    csrf: Option<&str>,
    name: &str,
    body: Option<&[u8]>,
) {
    if !csrf.is_some_and(|c| check_csrf(session, c)) {
        let error = json!({"error": "write from the page you are signed in on"});
        return json(stream, "403 Forbidden", &error);
    }
    let Some(NowBody { repo }) = body.and_then(|b| serde_json::from_slice(b).ok()) else {
        let error = json!({"error": "name the repo to run"});
        return json(stream, "400 Bad Request", &error);
    };
    let request = ControllerRequest::WorkflowRunNow(WorkflowRunNowReq {
        identity: None,
        name: decode(name),
        repos: vec![repo],
        expected_revision: None,
    });
    let reply = match tools::ask_as(&gate.controller, &session.person, request) {
        Ok(reply) => reply,
        Err(why) => return failed(stream, "run now", &why),
    };
    if reply.class != ExitClass::Ok {
        return answered(stream, &reply);
    }
    let value = reply.value.clone().unwrap_or_default();
    let Some(line) = serde_json::from_value::<RunNowValue>(value)
        .ok()
        .and_then(|v| v.runs.into_iter().next())
    else {
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
