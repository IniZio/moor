//! The machine settings route: one node's cap and labels replaced as the signed-in person. The
//! controller decides whose node it is; this carries the answer and tells a stale write from
//! other refusals.

use std::net::TcpStream;
use std::num::NonZeroU32;

use moor_core::request::{CapEdit, NodeSettingsGetReq, NodeSettingsSetReq, Request};
use moor_core::response::ExitClass;
use moor_core::view::node::NodeSettingsValue;
use serde::Deserialize;
use serde_json::json;

use super::account::{failed, json};
use super::repos::answered;
use super::{check_csrf, Session};
use crate::{tools, Gate};

/// What a node is written under: this, then its name.
const PREFIX: &str = "/account/api/nodes/";

/// The largest settings write the page reads.
pub(super) const BODY_MAX: usize = 8 * 1024;

/// The body of a settings write.
#[derive(Deserialize)]
struct Write {
    /// The most live Boxes the node holds; none for no cap.
    max_boxes: Option<u32>,
    /// The node's whole label list.
    labels: Vec<String>,
    /// The revision the page read, 0 when the settings were never written.
    revision: u64,
}

/// The node name in `path`, when it is a settings route.
pub(super) fn named(path: &str) -> Option<&str> {
    path.strip_prefix(PREFIX)
        .filter(|name| !name.is_empty() && !name.contains('/'))
}

/// Replaces the settings of the node `path` names as the session's person, when the request
/// carries the CSRF token. A write at a revision that is no longer the stored one answers `409`
/// with who changed it and when.
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
    let name = named(path).unwrap_or_default().to_owned();
    let parsed = body
        .and_then(|b| serde_json::from_slice::<Write>(b).ok())
        .filter(|w| w.max_boxes != Some(0));
    let (Some(write), false) = (parsed, name.is_empty()) else {
        let error = json!({"error": "name a node, and send its cap (at least 1, or null), labels and the revision you read"});
        return json(stream, "400 Bad Request", &error);
    };
    let ask = |request| tools::ask_as(&gate.controller, &session.person, request);
    let set = Request::NodeSettingsSet(NodeSettingsSetReq {
        identity: None,
        name: name.clone(),
        max_boxes: write
            .max_boxes
            .and_then(NonZeroU32::new)
            .map_or(CapEdit::Clear, CapEdit::Set),
        labels: Some(write.labels),
        expected_revision: Some(write.revision),
    });
    let reply = match ask(set) {
        Ok(reply) => reply,
        Err(why) => return failed(stream, "node settings write", &why),
    };
    if reply.class == ExitClass::Refused {
        let get = Request::NodeSettingsGet(NodeSettingsGetReq {
            identity: None,
            name,
        });
        let now = ask(get)
            .ok()
            .filter(|r| r.class == ExitClass::Ok)
            .and_then(|r| r.value)
            .and_then(|v| serde_json::from_value::<NodeSettingsValue>(v).ok());
        if let Some(now) = now.filter(|n| n.revision != write.revision) {
            return changed(stream, &now);
        }
    }
    answered(stream, &reply);
}

/// Answers `409`: the settings are no longer at the revision the page wrote against.
fn changed(stream: &mut TcpStream, now: &NodeSettingsValue) {
    let by = now.edited_by.as_deref().unwrap_or("someone");
    let at = now.edited_at.as_deref().unwrap_or("an unknown time");
    let error = format!("Changed by {by} at {at}. Reload to see it before you save.");
    let body = json!({"error": error, "current": {
        "revision": now.revision, "edited_by": now.edited_by, "edited_at": now.edited_at}});
    json(stream, "409 Conflict", &body);
}
