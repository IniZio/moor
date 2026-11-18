//! Replacing a workflow's prompt and plan as the page's person.

use std::net::TcpStream;

use moor_core::request::{Request as ControllerRequest, WorkflowAddReq, WorkflowEnableReq};
use moor_core::response::{ExitClass, Response as ControllerResponse};
use moor_core::view::workflow::WorkflowView;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{decode, read, workflows, Failure};
use crate::login::account::{failed, json};
use crate::login::repos::answered;
use crate::login::{check_csrf, Session};
use crate::{tools, Gate};

/// The body of a workflow write.
#[derive(Deserialize)]
struct Write {
    /// What each Box's agent is asked to do.
    prompt: String,
    /// The plan as JSON text; blank for none.
    plan: String,
    /// The revision the page read.
    revision: u64,
    /// Yes when the page asked the person and they agreed that a stored plan goes.
    #[serde(default)]
    clear_plan: bool,
}

/// Replaces the prompt and plan of workflow `name` as the session's person, when the request
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
        let error = json!({"error": "send the prompt, the plan and the revision you read"});
        return json(stream, "400 Bad Request", &error);
    };
    let name = decode(name);
    match save(gate, session, &name, &write) {
        Ok(()) => read(stream, gate, session, &name),
        Err(Saved::Conflict(now)) => changed(stream, now),
        Err(Saved::Bad(why)) => json(stream, "400 Bad Request", &why),
        Err(Saved::Refused(reply)) => answered(stream, &reply),
        Err(Saved::Other(why)) => failed(stream, "workflow write", &why),
    }
}

/// Why a save did not happen.
enum Saved {
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
            Failure::Other(why) => Self::Other(why),
        }
    }
}

/// A plan with `edited` and nothing else dropped, for comparing a person's change to the
/// stored plan.
fn without_edited(plan: &Value) -> Value {
    let mut plan = plan.clone();
    if let Some(map) = plan.as_object_mut() {
        map.remove("edited");
    }
    plan
}

/// Stores the written prompt and plan on top of what the workflow already is.
fn save(gate: &Gate, session: &Session, name: &str, write: &Write) -> Result<(), Saved> {
    let found = workflows(gate, session)?
        .into_iter()
        .find(|w| w.name == name);
    let Some(stored) = found else {
        return Err(Saved::Conflict(None));
    };
    if stored.revision != write.revision {
        return Err(Saved::Conflict(Some(stored.revision)));
    }
    let plan = plan_to_store(&stored, write)?;
    let add = ControllerRequest::WorkflowAdd(WorkflowAddReq {
        identity: None,
        name: stored.name.clone(),
        cron: stored.cron.clone(),
        timezone: stored.timezone.clone(),
        repos: stored.repos.clone(),
        also: stored.also.clone(),
        config: stored.config.clone(),
        prompt: write.prompt.clone(),
        timeout_minutes: Some(stored.timeout_minutes),
        plan,
        expected_revision: Some(write.revision),
        origin_key: None,
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
            .map_or("the controller refused the plan", |e| e.message.as_str());
        return Err(Saved::Bad(json!({"error": why})));
    }
    if reply.class != ExitClass::Ok {
        return Err(Saved::Refused(Box::new(reply)));
    }
    if !stored.enabled {
        // A replace enables a workflow; a person who only edited its plan did not ask for that.
        let off = ControllerRequest::WorkflowSetEnabled(WorkflowEnableReq {
            identity: None,
            name: stored.name,
            enabled: false,
        });
        tools::ask_as(&gate.controller, &session.person, off).map_err(Saved::Other)?;
    }
    Ok(())
}

/// The plan text to store for `write`: none when the page asked for none, and it said it
/// knows a stored plan goes; otherwise the written JSON, marked `edited` when it differs from
/// the stored plan.
fn plan_to_store(stored: &WorkflowView, write: &Write) -> Result<Option<String>, Saved> {
    let text = write.plan.trim();
    if text.is_empty() {
        if stored.plan.is_some() && !write.clear_plan {
            let prompt_changed = stored.prompt != write.prompt;
            let why = if prompt_changed {
                "This changes the prompt and leaves the plan empty, so the plan would be \
                 cleared and every run would deliver the prompt in one step."
            } else {
                "The plan is empty, so it would be cleared and every run would deliver the \
                 prompt in one step."
            };
            return Err(Saved::Bad(json!({"error": why, "code": "clears_plan"})));
        }
        return Ok(None);
    }
    let mut plan: Value = serde_json::from_str(text)
        .map_err(|e| Saved::Bad(json!({"error": format!("the plan is not JSON: {e}")})))?;
    let Some(map) = plan.as_object_mut() else {
        return Err(Saved::Bad(
            json!({"error": "the plan must be an object with a `steps` list"}),
        ));
    };
    let same = stored
        .plan
        .as_ref()
        .is_some_and(|p| without_edited(p) == without_edited(&Value::Object(map.clone())));
    let was_edited = stored
        .plan
        .as_ref()
        .and_then(|p| p.get("edited"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    map.insert("edited".to_owned(), json!(!same || was_edited));
    Ok(Some(plan.to_string()))
}

/// Answers `409`: the workflow is no longer at the revision the page wrote against.
fn changed(stream: &mut TcpStream, now: Option<u64>) {
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
