//! Replacing a workflow's prompt as the page's person. Its plan is moor's, never written here.

use std::net::TcpStream;

use moor_core::request::{Request as ControllerRequest, WorkflowAddReq};
use moor_core::response::{ExitClass, Response as ControllerResponse};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{decode, not_found, read, workflows, Failure};
use crate::login::account::{failed, json};
use crate::login::repos::answered;
use crate::login::{check_csrf, Session};
use crate::{tools, Gate};

/// The body of a workflow write.
#[derive(Deserialize)]
struct Write {
    /// What each Box's agent is asked to do.
    prompt: String,
    /// The revision the page read.
    revision: u64,
}

/// Replaces the prompt of workflow `name` as the session's person, when the request
/// carries the CSRF token. A write at a revision that is no longer the stored one answers `409`.
pub(crate) fn write(
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
    let Some(write) = body.and_then(|b| serde_json::from_slice::<Write>(b).ok()) else {
        let error = json!({"error": "send the prompt and the revision you read"});
        return json(stream, "400 Bad Request", &error);
    };
    let name = decode(name);
    match save(gate, session, &name, &write) {
        Ok(()) => read(stream, gate, session, &name),
        Err(Saved::Missing) => not_found(stream),
        Err(Saved::Conflict(now)) => changed(stream, now),
        Err(Saved::Bad(why)) => json(stream, "400 Bad Request", &why),
        Err(Saved::Refused(reply)) => answered(stream, &reply),
        Err(Saved::Other(why)) => failed(stream, "workflow write", &why),
    }
}

/// Why a save did not happen.
enum Saved {
    /// The workflow is not the person's: it does not exist or is someone else's.
    Missing,
    /// The workflow moved on; the revision it is at, none when it is gone.
    Conflict(Option<u64>),
    /// The write is wrong: a body for the page to show.
    Bad(Value),
    /// The controller refused.
    Refused(Box<ControllerResponse>),
    /// The controller could not be asked.
    Other(String),
}

impl From<Failure> for Saved {
    fn from(f: Failure) -> Self {
        match f {
            Failure::Reply(r) => Self::Refused(r),
            Failure::Missing => Self::Missing,
            Failure::Other(why) => Self::Other(why),
        }
    }
}

/// Stores the written prompt on top of what the workflow already is.
fn save(gate: &Gate, session: &Session, name: &str, write: &Write) -> Result<(), Saved> {
    let found = Failure::own(workflows(gate, session))?
        .into_iter()
        .find(|w| w.name == name);
    let Some(stored) = found else {
        return Err(Saved::Missing);
    };
    if stored.revision != write.revision {
        return Err(Saved::Conflict(Some(stored.revision)));
    }
    let add = ControllerRequest::WorkflowAdd(WorkflowAddReq {
        identity: None,
        name: stored.name.clone(),
        cron: Some(stored.cron.clone().unwrap_or_default()),
        timezone: stored.timezone.clone(),
        repos: stored.repos.clone(),
        config: stored.config.clone(),
        prompt: write.prompt.clone(),
        timeout_minutes: Some(stored.timeout_minutes),
        expected_revision: Some(write.revision),
        origin_key: None,
        // An edit of the content only: whether it runs, and why not, stay as stored, and so
        // does the origin.
        keep_state: true,
        enabled: None,
        on_pull_request: Some(stored.on_pull_request),
    });
    let reply = tools::ask_as(&gate.controller, &session.person, add).map_err(Saved::Other)?;
    if reply.class == ExitClass::Refused {
        let now = workflows(gate, session)?
            .into_iter()
            .find(|w| w.name == name);
        match now {
            None => return Err(Saved::Conflict(None)),
            Some(now) if now.revision != write.revision => {
                return Err(Saved::Conflict(Some(now.revision)));
            }
            Some(_) => {}
        }
    }
    if reply.class == ExitClass::Usage {
        let why = reply
            .error
            .as_ref()
            .map_or("the controller refused the prompt", |e| e.message.as_str());
        return Err(Saved::Bad(json!({"error": why})));
    }
    if reply.class != ExitClass::Ok {
        return Err(Saved::Refused(Box::new(reply)));
    }
    Ok(())
}

/// Answers `409`: the workflow is no longer at the revision the page wrote against.
pub(super) fn changed(stream: &mut TcpStream, now: Option<u64>) {
    let error = now.map_or_else(
        || "This workflow was removed. Reload to see what is left.".to_owned(),
        |_| {
            "Someone changed this workflow after you read it. Reload to see it before you save."
                .to_owned()
        },
    );
    json(
        stream,
        "409 Conflict",
        &json!({"error": error, "current": {"revision": now}}),
    );
}
