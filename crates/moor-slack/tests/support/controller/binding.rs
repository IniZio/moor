//! The fake controller's front bindings, kept as the real store keeps them: the first binding
//! of a (front, key) wins, and only a live Session can be bound.

use moor_core::request::Request;
use moor_core::response::Response;
use serde_json::json;

use super::{live_host, refused, FakeController};

/// A binding as the fake holds it: front, key, Session, owner.
pub(crate) type Bound = (String, String, i64, String);

impl FakeController {
    /// Every binding made so far, oldest first.
    pub(crate) fn bindings(&self) -> Vec<Bound> {
        self.lock().bindings.clone()
    }
}

/// The answer to `bind-session`; `None` for any other request.
pub(super) fn answer(
    request: &Request,
    hosts: &mut [(String, bool)],
    bindings: &mut Vec<Bound>,
) -> Option<Response> {
    let Request::BindSession(bind) = request else {
        return None;
    };
    let session = bind.session.get();
    if let Err(message) = live_host(hosts, session) {
        return Some(refused("bind-session", &message));
    }
    let held = bindings
        .iter()
        .find(|(front, key, ..)| *front == bind.front && *key == bind.key)
        .cloned();
    let created = held.is_none();
    let (front, key, session, owner) = held.unwrap_or_else(|| {
        let new = (
            bind.front.clone(),
            bind.key.clone(),
            session,
            bind.owner.clone(),
        );
        bindings.push(new.clone());
        new
    });
    let binding =
        json!({"front": front, "key": key, "session": session, "box": null, "owner": owner});
    let value = json!({"created": created, "binding": binding});
    Some(Response::ok("bind-session", Some(value)))
}
