//! The settings routes a person sets themselves up with: a one-use token that enrolls a machine,
//! removing a machine, and the agent profile, checked through GitHub before it is saved. Each
//! acts as the signed-in person; each write needs the session's CSRF token.

use std::net::TcpStream;

use moor_core::name::ProfileSource;
use moor_core::request::{
    AsReq, MachineRemoveReq, PairOp, PairOwner, ProfileCheckReq, ProfileSetReq,
    Request as ControllerRequest,
};
use moor_core::response::{ExitClass, Response as ControllerResponse};
use moor_core::view::user::{MachinePairValue, ProfileCheckValue};
use oauth2::CsrfToken;
use serde::Deserialize;
use serde_json::{json, Value};

use super::account::{failed, json, signed_out};
use super::repos::{self, answered};
use super::routes::{read_body, Request};
use super::{check_csrf, Session, RANDOM_BYTES};
use crate::{tools, Gate};

/// Where a machine is removed: this, then its name.
const MACHINES: &str = "/account/api/machines/";
/// Where a token is made.
const TOKEN: &str = "/account/api/machines/token";
/// The profile, read and written.
const PROFILE: &str = "/account/api/profile";
/// A profile checked without saving it.
const CHECK: &str = "/account/api/profile/check";
/// The operator's GitHub App: its install page and the repos it is installed on, or null.
const GITHUB_APP: &str = "/account/api/github-app";

/// How long a machine token can be redeemed.
const TOKEN_TTL_SECS: u64 = 600;

/// The most a settings write holds.
const BODY_MAX: usize = 4096;

/// The body of a profile write or check.
#[derive(Deserialize)]
struct Source {
    /// `https://github.com/OWNER/NAME[#REF]`.
    source: String,
}

/// Whether the request is for a settings route.
pub(super) fn serves(method: &str, path: &str) -> bool {
    match method {
        "POST" => path == TOKEN || path == CHECK,
        "GET" => path == PROFILE || path == GITHUB_APP,
        "PUT" => path == PROFILE,
        "DELETE" => path == PROFILE || machine(path).is_some(),
        _ => false,
    }
}

/// The machine name in `path`, when it names one.
fn machine(path: &str) -> Option<&str> {
    path.strip_prefix(MACHINES)
        .filter(|name| !name.is_empty() && !name.contains('/') && *name != "token")
}

/// Answers a settings route (see [`serves`]) for the signed-in person.
pub(super) fn serve(
    stream: &mut TcpStream,
    req: &Request<'_>,
    gate: &Gate,
    session: Option<&Session>,
) {
    let path = req.target.split('?').next().unwrap_or_default();
    // Read the body before any answer: closing on unread bytes resets the connection.
    let body = (req.method != "GET")
        .then(|| read_body(stream, req.leftover.clone(), req.length, BODY_MAX))
        .flatten();
    let Some(session) = session else {
        return signed_out(stream);
    };
    if req.method != "GET" && !req.csrf.is_some_and(|c| check_csrf(session, c)) {
        let error = json!({"error": "write from the page you are signed in on"});
        return json(stream, "403 Forbidden", &error);
    }
    let ask = |request| tools::ask_as(&gate.controller, &session.person, request);
    let reply = match (req.method, path) {
        ("POST", TOKEN) => return token(stream, gate, session),
        ("GET", GITHUB_APP) => github_app(&ask, gate, session),
        ("GET", _) => ask(check(None)),
        ("POST", _) | ("PUT", PROFILE) => match source(body.as_deref()) {
            Ok(source) if req.method == "POST" => ask(check(Some(source))),
            Ok(source) => return save(stream, &ask, source),
            Err(why) => return json(stream, "400 Bad Request", &json!({"error": why})),
        },
        ("DELETE", PROFILE) => ask(ControllerRequest::ProfileRemove(AsReq { identity: None })),
        _ => ask(ControllerRequest::MachineRemove(MachineRemoveReq {
            identity: None,
            name: machine(path).unwrap_or_default().to_owned(),
        })),
    };
    match reply {
        Ok(reply) => answered(stream, &reply),
        Err(why) => failed(stream, "settings", &why),
    }
}

/// The App's answer with its repos cut to the ones the person has in moor, so a repo the App
/// serves for someone else never leaves the gate. A failed read of their repos fails the answer.
fn github_app(
    ask: &impl Fn(ControllerRequest) -> Result<ControllerResponse, String>,
    gate: &Gate,
    session: &Session,
) -> Result<ControllerResponse, String> {
    let mut reply = ask(ControllerRequest::GitHubApp(AsReq { identity: None }))?;
    if reply.class != ExitClass::Ok {
        return Ok(reply);
    }
    if let Some(app) = reply.value.as_mut().filter(|v| !v.is_null()) {
        let theirs = repos::used(gate, session)?;
        let kept: Vec<Value> = app["repos"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| r.as_str().is_some_and(|r| theirs.contains(r)))
            .cloned()
            .collect();
        app["repos"] = Value::Array(kept);
    }
    Ok(reply)
}

/// The profile check request for `source`, or for the stored profile.
const fn check(source: Option<ProfileSource>) -> ControllerRequest {
    ControllerRequest::ProfileCheck(ProfileCheckReq {
        identity: None,
        source,
    })
}

/// The profile source a write carries, or why it carries none.
fn source(body: Option<&[u8]>) -> Result<ProfileSource, String> {
    let sent = body
        .and_then(|b| serde_json::from_slice::<Source>(b).ok())
        .ok_or("send the profile as source")?;
    ProfileSource::parse(sent.source.trim()).map_err(|e| format!("{e}."))
}

/// Checks `source` and saves it only when every step passed, answering the check either way:
/// `400` with the first failure when it did not pass.
fn save(
    stream: &mut TcpStream,
    ask: &dyn Fn(ControllerRequest) -> Result<moor_core::Response, String>,
    source: ProfileSource,
) {
    let checked = match ask(check(Some(source.clone()))) {
        Ok(reply) if reply.class == ExitClass::Ok => reply.value.unwrap_or_default(),
        Ok(reply) => return answered(stream, &reply),
        Err(why) => return failed(stream, "profile check", &why),
    };
    let found = serde_json::from_value::<ProfileCheckValue>(checked.clone()).ok();
    let why = found.as_ref().map_or_else(
        || Some("moor answered the check in a shape the gate does not know".to_owned()),
        |c| c.failures.first().map(|f| f.detail.clone()),
    );
    if let Some(why) = why {
        let error = format!("Not saved: {why}");
        return json(
            stream,
            "400 Bad Request",
            &json!({"error": error, "check": checked}),
        );
    }
    let set = ControllerRequest::ProfileSet(ProfileSetReq {
        identity: None,
        source,
    });
    match ask(set) {
        Ok(reply) if reply.class == ExitClass::Ok => json(stream, "200 OK", &checked),
        Ok(reply) => answered(stream, &reply),
        Err(why) => failed(stream, "profile write", &why),
    }
}

/// Makes a one-use token that enrolls a machine as the session's person, replacing any earlier
/// one. Only its hash is kept.
fn token(stream: &mut TcpStream, gate: &Gate, session: &Session) {
    let token = CsrfToken::new_random_len(RANDOM_BYTES).into_secret();
    let op = PairOp::Grant {
        owner: PairOwner {
            identity: session.person.identity.clone(),
            email: session.person.email.clone(),
        },
        token: token.clone(),
        ttl_secs: TOKEN_TTL_SECS,
    };
    match super::pair::step(&gate.controller, op) {
        Ok(MachinePairValue::Started) => json(
            stream,
            "200 OK",
            &json!({"token": token, "expires_in": TOKEN_TTL_SECS}),
        ),
        Ok(MachinePairValue::Full) => json(
            stream,
            "429 Too Many Requests",
            &json!({"error": "too many machines are pairing; try again in a few minutes"}),
        ),
        Ok(other) => failed(stream, "machine token", &format!("moor answered {other:?}")),
        Err(why) => failed(stream, "machine token", &why),
    }
}

#[cfg(test)]
mod tests;
