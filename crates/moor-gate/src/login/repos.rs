//! The repo settings page's data: the repos a person has used, and one repo's config read and
//! replaced as them. The controller decides who may read and who may push; this only carries
//! the answer, and tells a stale write from other refusals.

use std::collections::BTreeSet;
use std::net::TcpStream;

use moor_core::name::ForgeRepo;
use moor_core::request::{
    RepoConfigGetReq, RepoConfigSetReq, Request as ControllerRequest, ShowReq,
};
use moor_core::response::{ExitClass, Response as ControllerResponse};
use moor_core::view::repo_config::RepoConfigValue;
use serde::Deserialize;
use serde_json::{json, Value};

use super::account::{failed, json};
use super::{check_csrf, Session};
use crate::{tools, Gate};

/// What a repo is read and written under: this, then `owner/name`.
const PREFIX: &str = "/account/api/repos/";

/// The largest config write the page reads: the controller's own cap, with room for JSON escapes.
pub(super) const BODY_MAX: usize = 256 * 1024;

/// The body of a config write.
#[derive(Deserialize)]
struct Write {
    /// The whole config as YAML.
    text: String,
    /// The revision the page read, 0 when the repo had none.
    revision: u64,
}

/// The `owner/name` in `path`, when it is a repo route.
pub(super) fn named(path: &str) -> Option<&str> {
    path.strip_prefix(PREFIX)
        .filter(|rest| rest.split('/').count() == 2)
}

/// Answers the repos of the person's Boxes: the ones they have used, that is.
pub(super) fn list(stream: &mut TcpStream, gate: &Gate, session: &Session) {
    match used(gate, session) {
        Ok(repos) => json(stream, "200 OK", &json!({"repos": repos})),
        Err(why) => failed(stream, "repo list", &why),
    }
}

/// The distinct repos of the person's Boxes, sorted: the repos they have in moor.
pub(super) fn used(gate: &Gate, session: &Session) -> Result<BTreeSet<String>, String> {
    let show = ControllerRequest::Show(ShowReq {
        target: None,
        events: None,
        agent: false,
        reports: false,
        artifacts: false,
        pane: false,
        transcript: false,
    });
    let shown = tools::act_as(&gate.controller, &session.person, show)?;
    Ok(repos_of(&shown).into_iter().map(str::to_owned).collect())
}

/// The distinct repos of the Boxes in a `show` list, sorted.
fn repos_of(shown: &Value) -> BTreeSet<&str> {
    let rows = shown.get("boxes").and_then(Value::as_array);
    rows.into_iter()
        .flatten()
        .flat_map(|row| {
            let others = row.get("other_repos").and_then(Value::as_array);
            row.get("repo")
                .into_iter()
                .chain(others.into_iter().flatten())
        })
        .filter_map(Value::as_str)
        .collect()
}

/// Answers the stored config of the repo `path` names, and whether the person may edit it.
pub(super) fn read(stream: &mut TcpStream, gate: &Gate, session: &Session, path: &str) {
    let Some(repo) = named(path).and_then(|p| ForgeRepo::parse(p).ok()) else {
        return not_a_repo(stream);
    };
    let get = ControllerRequest::RepoConfigGet(RepoConfigGetReq {
        identity: None,
        repo,
    });
    match tools::ask_as(&gate.controller, &session.person, get) {
        Ok(reply) => answered(stream, &reply),
        Err(why) => failed(stream, "repo config read", &why),
    }
}

/// Replaces the config of the repo `path` names with the written text, as the session's person,
/// when the request carries the CSRF token. A write at a revision that is no longer the stored
/// one answers `409` with who changed it and when.
pub(super) fn write(
    stream: &mut TcpStream,
    gate: &Gate,
    session: &Session,
    csrf: Option<&str>,
    path: &str,
    body: Option<&[u8]>,
) {
    if !csrf.is_some_and(|c| check_csrf(session, c)) {
        let error = json!({"error": "write from the page you are signed in on"});
        return json(stream, "403 Forbidden", &error);
    }
    let Some(repo) = named(path).and_then(|p| ForgeRepo::parse(p).ok()) else {
        return not_a_repo(stream);
    };
    let Some(write) = body.and_then(|b| serde_json::from_slice::<Write>(b).ok()) else {
        let error = json!({"error": "send the config text and the revision you read"});
        return json(stream, "400 Bad Request", &error);
    };
    let ask = |request| tools::ask_as(&gate.controller, &session.person, request);
    let set = ControllerRequest::RepoConfigSet(RepoConfigSetReq {
        identity: None,
        repo: repo.clone(),
        text: write.text,
        expected_revision: Some(write.revision),
    });
    let reply = match ask(set) {
        Ok(reply) => reply,
        Err(why) => return failed(stream, "repo config write", &why),
    };
    if reply.class == ExitClass::Refused {
        let get = ControllerRequest::RepoConfigGet(RepoConfigGetReq {
            identity: None,
            repo,
        });
        if let Some(now) = ask(get).ok().and_then(|r| current(&r)) {
            if now.revision != write.revision {
                return changed(stream, &now);
            }
        }
    }
    answered(stream, &reply);
}

/// The config in a controller reply that read it.
fn current(reply: &ControllerResponse) -> Option<RepoConfigValue> {
    let value = reply
        .value
        .clone()
        .filter(|_| reply.class == ExitClass::Ok)?;
    serde_json::from_value(value).ok()
}

/// Answers `409`: the config is no longer at the revision the page wrote against.
fn changed(stream: &mut TcpStream, now: &RepoConfigValue) {
    let by = now.edited_by.as_deref().unwrap_or("someone");
    let at = now.edited_at.as_deref().unwrap_or("an unknown time");
    let error = format!("Changed by {by} at {at}. Reload to see it before you save.");
    let body = json!({"error": error, "current": {
        "revision": now.revision, "edited_by": now.edited_by, "edited_at": now.edited_at}});
    json(stream, "409 Conflict", &body);
}

/// Answers `reply` as the page reads it: its value, or the controller's reason with a status
/// that says whose doing it is.
pub(super) fn answered(stream: &mut TcpStream, reply: &ControllerResponse) {
    let status = match reply.class {
        ExitClass::Ok => {
            let value = reply.value.clone().unwrap_or_default();
            return json(stream, "200 OK", &value);
        }
        ExitClass::Usage => "400 Bad Request",
        ExitClass::Refused => "403 Forbidden",
        ExitClass::Failed => "502 Bad Gateway",
    };
    let why = reply
        .error
        .as_ref()
        .map_or("the controller refused", |e| e.message.as_str());
    json(stream, status, &json!({"error": why}));
}

/// Answers `400` for a path that does not name a repo.
fn not_a_repo(stream: &mut TcpStream) {
    json(
        stream,
        "400 Bad Request",
        &json!({"error": "name a repo as owner/name"}),
    );
}
