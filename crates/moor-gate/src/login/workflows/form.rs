//! A workflow made, edited, switched on or off, or removed from the portal's form, as the
//! session's person, through the same requests `add_workflow` and its siblings send.

use std::net::TcpStream;

use moor_core::request::{
    Request as ControllerRequest, WorkflowAddReq, WorkflowEnableReq, WorkflowRemoveReq,
};
use moor_core::response::{ExitClass, Response as ControllerResponse};
use moor_core::view::workflow::WorkflowView;
use serde::Deserialize;
use serde_json::{json, Value};

use super::save::changed;
use super::{decode, not_found, read, workflows, Failure};
use crate::login::account::{failed, json};
use crate::login::repos::answered;
use crate::login::{check_csrf, Session};
use crate::{tools, Gate};

/// What the form sends for a workflow: everything a person sets in it.
#[derive(Deserialize)]
struct Fields {
    /// The repos, each `owner/name`, first one first.
    repos: Vec<String>,
    /// What each run's agent is asked to do.
    prompt: String,
    /// Five-field cron expression; empty or absent for a workflow with no cron.
    #[serde(default)]
    cron: String,
    /// IANA time zone the cron is read in.
    #[serde(default)]
    timezone: String,
    /// Minutes a run may take; the stored value, or the default for a new workflow, when absent.
    #[serde(default)]
    timeout_minutes: Option<u32>,
    /// Whether a pull request event on the first repo fires a run; the stored value when absent.
    #[serde(default)]
    on_pull_request: Option<bool>,
}

/// The body of a create.
#[derive(Deserialize)]
struct Create {
    /// The new workflow's name.
    name: String,
    /// The fields it starts with.
    #[serde(flatten)]
    fields: Fields,
    /// A workflow of the person's whose plan and config the new one copies.
    #[serde(default)]
    copy_of: Option<String>,
    /// The plan's JSON text, such as one an agent drafted; it replaces a copied plan. None when
    /// absent or blank.
    #[serde(default)]
    plan: Option<String>,
    /// False to store the new workflow disabled; true when absent.
    #[serde(default = "on")]
    enabled: bool,
}

/// The default of a create's `enabled`.
const fn on() -> bool {
    true
}

/// The body of an edit.
#[derive(Deserialize)]
struct Edit {
    /// The fields it ends up with.
    #[serde(flatten)]
    fields: Fields,
    /// The revision the page read.
    revision: u64,
}

/// The body of a switch on or off.
#[derive(Deserialize)]
struct Switch {
    /// Whether the workflow fires on its schedule.
    enabled: bool,
}

/// Answers `403` unless the request carries the session's CSRF token.
fn forbidden(stream: &mut TcpStream, session: &Session, csrf: Option<&str>) -> bool {
    if csrf.is_some_and(|c| check_csrf(session, c)) {
        return false;
    }
    let error = json!({"error": "write from the page you are signed in on"});
    json(stream, "403 Forbidden", &error);
    true
}

/// The body as `T`, or a `400` answered.
fn parsed<T: serde::de::DeserializeOwned>(
    stream: &mut TcpStream,
    body: Option<&[u8]>,
) -> Option<T> {
    let parsed = body.and_then(|b| serde_json::from_slice::<T>(b).ok());
    if parsed.is_none() {
        let error = json!({"error": "send the name, repos, prompt, schedule and time zone"});
        json(stream, "400 Bad Request", &error);
    }
    parsed
}

/// The person's workflow `name`, or what to answer instead.
fn stored(gate: &Gate, session: &Session, name: &str) -> Result<Option<WorkflowView>, Failure> {
    Ok(Failure::own(workflows(gate, session))?
        .into_iter()
        .find(|w| w.name == name))
}

/// Answers a failed read of the person's workflows.
fn failure(stream: &mut TcpStream, f: Failure) {
    match f {
        Failure::Reply(reply) => answered(stream, &reply),
        Failure::Missing => not_found(stream),
        Failure::Other(why) => failed(stream, "workflow write", &why),
    }
}

/// `text` trimmed, or none when it is blank.
fn non_empty(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// The add request for `name` with `fields`, the plan and config of `kept`.
fn add(name: &str, fields: Fields, kept: Option<&WorkflowView>) -> WorkflowAddReq {
    let on_pull_request = fields
        .on_pull_request
        .unwrap_or_else(|| kept.is_some_and(|w| w.on_pull_request));
    WorkflowAddReq {
        identity: None,
        name: name.to_owned(),
        cron: Some(non_empty(&fields.cron).unwrap_or_default()),
        timezone: non_empty(&fields.timezone),
        repos: fields.repos.iter().map(|r| r.trim().to_owned()).collect(),
        config: kept.and_then(|w| w.config.clone()),
        prompt: fields.prompt,
        timeout_minutes: fields.timeout_minutes,
        plan: kept.and_then(|w| w.plan.as_ref()).map(Value::to_string),
        expected_revision: None,
        origin_key: None,
        keep_state: false,
        enabled: Some(true),
        on_pull_request: Some(on_pull_request),
        gates: None,
        outcomes: None,
    }
}

/// Answers the controller's reply to an add: the workflow as stored, or why not.
fn added(
    stream: &mut TcpStream,
    gate: &Gate,
    session: &Session,
    name: &str,
    reply: &ControllerResponse,
) {
    match reply.class {
        ExitClass::Ok => read(stream, gate, session, name),
        ExitClass::Usage => {
            let why = reply
                .error
                .as_ref()
                .map_or("the controller refused it", |e| e.message.as_str());
            json(stream, "400 Bad Request", &json!({"error": why}));
        }
        _ => answered(stream, reply),
    }
}

/// Makes a workflow as the session's person. A name already taken is refused, so a create never
/// replaces a workflow. A copy takes the plan and config of the one it copies.
pub(crate) fn create(
    stream: &mut TcpStream,
    gate: &Gate,
    session: &Session,
    csrf: Option<&str>,
    body: Option<&[u8]>,
) {
    if forbidden(stream, session, csrf) {
        return;
    }
    let Some(new) = parsed::<Create>(stream, body) else {
        return;
    };
    let all = match workflows(gate, session) {
        Ok(all) => all,
        Err(f) => return failure(stream, f),
    };
    let name = new.name.trim().to_owned();
    if all.iter().any(|w| w.name == name) {
        let error = format!("You already have a workflow named {name}. Pick another name.");
        return json(stream, "409 Conflict", &json!({"error": error}));
    }
    let copied = new
        .copy_of
        .as_deref()
        .map(|c| all.iter().find(|w| w.name == c));
    if copied.is_some_and(|c| c.is_none()) {
        return not_found(stream);
    }
    let mut request = add(&name, new.fields, copied.flatten());
    if let Some(plan) = new.plan.filter(|p| !p.trim().is_empty()) {
        request.plan = Some(plan);
    }
    request.enabled = Some(new.enabled);
    match tools::ask_as(
        &gate.controller,
        &session.person,
        ControllerRequest::WorkflowAdd(request),
    ) {
        Ok(reply) => added(stream, gate, session, &name, &reply),
        Err(why) => failed(stream, "workflow create", &why),
    }
}

/// Replaces the fields of workflow `name` as the session's person, keeping its plan, config,
/// and whether it runs. A write at a revision that is no longer the stored one answers `409`.
pub(crate) fn edit(
    stream: &mut TcpStream,
    gate: &Gate,
    session: &Session,
    csrf: Option<&str>,
    name: &str,
    body: Option<&[u8]>,
) {
    if forbidden(stream, session, csrf) {
        return;
    }
    let Some(edit) = parsed::<Edit>(stream, body) else {
        return;
    };
    let name = decode(name);
    let found = match stored(gate, session, &name) {
        Ok(Some(found)) => found,
        Ok(None) => return not_found(stream),
        Err(f) => return failure(stream, f),
    };
    if found.revision != edit.revision {
        return changed(stream, Some(found.revision));
    }
    let mut request = add(&name, edit.fields, Some(&found));
    request.expected_revision = Some(edit.revision);
    request.keep_state = true;
    let reply = match tools::ask_as(
        &gate.controller,
        &session.person,
        ControllerRequest::WorkflowAdd(request),
    ) {
        Ok(reply) => reply,
        Err(why) => return failed(stream, "workflow edit", &why),
    };
    if reply.class == ExitClass::Refused {
        let now = stored(gate, session, &name)
            .ok()
            .flatten()
            .map(|w| w.revision);
        if now != Some(edit.revision) {
            return changed(stream, now);
        }
    }
    added(stream, gate, session, &name, &reply);
}

/// Switches workflow `name` on or off as the session's person.
pub(crate) fn switch(
    stream: &mut TcpStream,
    gate: &Gate,
    session: &Session,
    csrf: Option<&str>,
    name: &str,
    body: Option<&[u8]>,
) {
    if forbidden(stream, session, csrf) {
        return;
    }
    let Some(switch) = parsed::<Switch>(stream, body) else {
        return;
    };
    let request = ControllerRequest::WorkflowSetEnabled(WorkflowEnableReq {
        identity: None,
        name: decode(name),
        enabled: switch.enabled,
    });
    match tools::ask_as(&gate.controller, &session.person, request) {
        Ok(reply) if reply.class == ExitClass::Ok => read(stream, gate, session, name),
        Ok(reply) if reply.class == ExitClass::Refused => not_found(stream),
        Ok(reply) => answered(stream, &reply),
        Err(why) => failed(stream, "workflow switch", &why),
    }
}

/// Removes workflow `name` as the session's person.
pub(crate) fn remove(
    stream: &mut TcpStream,
    gate: &Gate,
    session: &Session,
    csrf: Option<&str>,
    name: &str,
) {
    if forbidden(stream, session, csrf) {
        return;
    }
    let name = decode(name);
    let request = ControllerRequest::WorkflowRemove(WorkflowRemoveReq {
        identity: None,
        name: name.clone(),
    });
    match tools::ask_as(&gate.controller, &session.person, request) {
        Ok(reply) if reply.class == ExitClass::Ok => {
            json(stream, "200 OK", &json!({"removed": name}));
        }
        Ok(reply) if reply.class == ExitClass::Refused => not_found(stream),
        Ok(reply) => answered(stream, &reply),
        Err(why) => failed(stream, "workflow remove", &why),
    }
}
