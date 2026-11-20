//! A Box's changes in git for the portal's Changes tab. The controller answers a live read for a
//! Box that is up and the diff it kept for one that sleeps or closed, so the tab never wakes a Box.

use std::net::TcpStream;

use moor_core::name::BoxName;
use moor_core::request::{ChangesReq, Request as ControllerRequest};
use moor_core::response::ExitClass;
use serde_json::json;

use super::super::account::{failed, json};
use super::super::repos::answered;
use super::super::Session;
use super::param;
use crate::{tools, Gate};

/// Answers the changes of the Box the query names, as the session's person.
pub(super) fn changes(stream: &mut TcpStream, gate: &Gate, session: &Session, query: &str) {
    let Some(name) = param(query, "box").and_then(|b| BoxName::parse_allow_bare(&b).ok()) else {
        return json(stream, "400 Bad Request", &json!({"error": "name a Box"}));
    };
    let request = ControllerRequest::Changes(ChangesReq {
        target: name.into(),
    });
    match tools::ask_as(&gate.controller, &session.person, request) {
        // Every Box the controller shows a person is theirs, so a refusal is no such Box.
        Ok(reply) if !matches!(reply.class, ExitClass::Ok | ExitClass::Usage) => {
            json(stream, "404 Not Found", &json!({"error": "no such Box"}));
        }
        Ok(reply) => answered(stream, &reply),
        Err(why) => failed(stream, "box changes", &why),
    }
}
