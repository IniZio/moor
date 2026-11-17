//! `node-add`: the operator enrolls a machine no user owns, which is a shared node.

use moor_core::config::{EnrolledMachine, BOXES_HOST};
use moor_core::name::NodeName;
use moor_core::request::NodeAddReq;
use moor_core::view::node::NodeAddValue;
use moor_core::ErrorKind;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::node::configured;

/// Adds the shared machine `req` names, or replaces the key and labels of the one of that name,
/// which keeps its id.
///
/// A node is named for its machine, so the name must be free: a `nodes:` entry or a user's machine
/// of the same name would make two nodes of one name.
///
/// # Errors
/// The name or key is not accepted, the name is taken, or a store failure.
pub(crate) fn run(cx: &Cx, req: &NodeAddReq) -> Result<NodeAddValue, CommandError> {
    let usage = |why: String| CommandError::new(ErrorKind::Usage, why);
    let name = NodeName::parse(&req.name).map_err(|e| usage(e.to_string()))?;
    let machine = EnrolledMachine::parse(name.as_str(), &req.pubkey, &req.labels, 1)
        .and_then(|m| m.with_pool(BOXES_HOST))
        .map_err(usage)?;
    let taken = |what: &str| {
        CommandError::new(
            ErrorKind::Refused,
            format!("{what} is already named {name}; pick another name"),
        )
    };
    if configured(cx.config()).0.contains_key(name.as_str()) {
        return Err(taken("a node in the user config"));
    }
    let store = cx.store()?;
    if store.user_machine_named(name.as_str())? {
        return Err(taken("a user's machine"));
    }
    let machine_id = store.enroll_shared_machine(&machine)?;
    Ok(NodeAddValue {
        machine_id,
        name: name.to_string(),
    })
}

#[cfg(test)]
mod tests;
