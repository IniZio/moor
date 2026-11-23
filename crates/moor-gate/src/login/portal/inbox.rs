//! The inbox the top bar shows: the signed-in person's list, marking items read, and clearing them.

use std::net::TcpStream;

use moor_core::request::{InboxClear, InboxOp, InboxReq, Request as ControllerRequest};
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

/// What a clear names: `{ids}` for items, or `{through}` for every item up to the newest seen.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Clear {
    /// The items to clear.
    ids: Option<Vec<i64>>,
    /// The newest item the person saw.
    through: Option<i64>,
}

/// The clear `body` names: one of its two shapes, with at least one id; none for anything else, so
/// a malformed body never clears everything.
fn clear_of(body: &[u8]) -> Option<InboxClear> {
    match serde_json::from_slice::<Clear>(body).ok()? {
        Clear {
            ids: Some(ids),
            through: None,
        } if !ids.is_empty() => Some(InboxClear::Ids(ids)),
        Clear {
            ids: None,
            through: Some(through),
        } => Some(InboxClear::Through(through)),
        _ => None,
    }
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

/// Clears the items the body names, or every item up to the newest the person saw, and answers
/// the inbox.
pub(super) fn clear(
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
    let Some(which) = body.and_then(clear_of) else {
        let error = json!({"error": "name the items to clear as {ids} or {through}"});
        return json(stream, "400 Bad Request", &error);
    };
    ask(stream, gate, session, InboxOp::Clear(which));
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
    fn a_clear_names_items_or_the_newest_seen_and_nothing_else() {
        assert_eq!(clear_of(br#"{"ids": [3]}"#), Some(InboxClear::Ids(vec![3])));
        assert_eq!(clear_of(br#"{"through": 9}"#), Some(InboxClear::Through(9)));
        for bad in [
            &br#"{"ids": []}"#[..],
            br"{}",
            br#"{"ids": [3], "through": 9}"#,
            br#"{"all": true}"#,
            b"not json",
        ] {
            assert_eq!(clear_of(bad), None, "{}", String::from_utf8_lossy(bad));
        }
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
