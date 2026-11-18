//! The inbox the top bar shows: the signed-in person's list, and marking items read.

use std::net::TcpStream;

use moor_core::request::{InboxOp, InboxReq, Request as ControllerRequest};
use moor_core::response::ExitClass;
use serde::Deserialize;
use serde_json::json;

use super::super::account::{failed, json};
use super::super::repos::answered;
use super::super::{check_csrf, Session};
use crate::{tools, Gate};

/// What a read-mark names.
#[derive(Deserialize)]
struct Mark {
    /// The items to mark; every item when absent or empty.
    #[serde(default)]
    ids: Vec<i64>,
}

/// The request for one inbox `op`, as the session's person.
const fn request(op: InboxOp) -> ControllerRequest {
    ControllerRequest::Inbox(InboxReq { identity: None, op })
}

/// The inbox operation `op` as the session's person, answered as the inbox it leaves.
fn ask(stream: &mut TcpStream, gate: &Gate, session: &Session, op: InboxOp) {
    match tools::ask_as(&gate.controller, &session.person, request(op)) {
        Ok(reply) if reply.class == ExitClass::Ok => {
            json(stream, "200 OK", &reply.value.unwrap_or_default());
        }
        Ok(reply) => answered(stream, &reply),
        Err(why) => failed(stream, "inbox", &why),
    }
}

/// Answers the person's inbox, newest first.
pub(super) fn list(stream: &mut TcpStream, gate: &Gate, session: &Session) {
    ask(stream, gate, session, InboxOp::List);
}

/// Marks the items the body names read, or all of them, and answers the inbox.
pub(super) fn read(
    stream: &mut TcpStream,
    gate: &Gate,
    session: &Session,
    csrf: Option<&str>,
    body: Option<&[u8]>,
) {
    if !csrf.is_some_and(|c| check_csrf(session, c)) {
        let error = json!({"error": "write from the page you are signed in on"});
        return json(stream, "403 Forbidden", &error);
    }
    let ids = body.map_or_else(Vec::new, |b| {
        serde_json::from_slice::<Mark>(b).map_or_else(|_| Vec::new(), |m| m.ids)
    });
    ask(stream, gate, session, InboxOp::Read { ids });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marking_names_items_or_none_for_all() {
        let named: Mark = serde_json::from_str(r#"{"ids": [3, 5]}"#).unwrap();
        assert_eq!(named.ids, [3, 5]);
        let all: Mark = serde_json::from_str("{}").unwrap();
        assert!(all.ids.is_empty());
    }

    #[test]
    fn the_requests_act_as_the_signed_in_person() {
        let want = ControllerRequest::Inbox(InboxReq {
            identity: None,
            op: InboxOp::List,
        });
        assert_eq!(request(InboxOp::List), want);
    }
}
