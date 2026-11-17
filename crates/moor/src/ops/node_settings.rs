//! `node-settings-get` and `node-settings-set`: a machine node's cap and labels as the store
//! keeps them.
//!
//! A personal node's owner edits it. The operator edits a shared node a machine agent provides.
//! A node the controller's config lists is edited in that config, and reads here as it says.

use moor_core::config::UserMachine;
use moor_core::name::NodeName;
use moor_core::request::{NodeSettingsGetReq, NodeSettingsSetReq};
use moor_core::user::Viewer;
use moor_core::view::node::{NodeScope, NodeSettingsValue};
use moor_core::ErrorKind;

use super::common::node::configured;
use super::common::promote::promote_detached;
use super::common::view::rfc3339;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{Change, NodeSettings, SettingsWrote};

/// A node the caller named.
enum Found {
    /// A machine node, whose settings the store keeps.
    Machine {
        /// Its row.
        row: NodeSettings,
        /// Whether the caller may change it.
        can_edit: bool,
    },
    /// A node the controller's config lists.
    Configured(moor_core::config::NodeConfig),
}

/// `node-settings-get`: the node's cap and labels, and whether the caller may change them.
///
/// # Errors
/// Refused when the caller sees no node of that name; a store failure.
pub(crate) fn get(cx: &Cx, req: &NodeSettingsGetReq) -> Result<NodeSettingsValue, CommandError> {
    Ok(match find(cx, &req.name)? {
        Found::Machine { row, can_edit } => value(&req.name, &row, can_edit),
        Found::Configured(config) => NodeSettingsValue {
            name: req.name.clone(),
            scope: NodeScope::Shared,
            max_boxes: config.max_boxes.map(std::num::NonZero::get),
            labels: config.labels,
            revision: 0,
            edited_by: None,
            edited_at: None,
            can_edit: false,
        },
    })
}

/// `node-settings-set`: changes the node's cap or labels, which the next placement reads, and
/// starts the Boxes the change makes room for.
///
/// # Errors
/// Refused when the caller may not change the node or `expected_revision` is not the stored
/// one; Usage for a label that is not accepted; a store failure.
pub(crate) fn set(cx: &Cx, req: &NodeSettingsSetReq) -> Result<NodeSettingsValue, CommandError> {
    let (row, can_edit) = match find(cx, &req.name)? {
        Found::Machine { row, can_edit } => (row, can_edit),
        Found::Configured(_) => {
            return Err(CommandError::new(
                ErrorKind::Refused,
                format!(
                    "{} is listed in the controller's config; change it there",
                    req.name
                ),
            ))
        }
    };
    if !can_edit {
        return Err(CommandError::new(
            ErrorKind::Refused,
            format!(
                "{} is a shared node; only the operator changes it",
                req.name
            ),
        ));
    }
    let labels = req
        .labels
        .as_deref()
        .map(|labels| UserMachine::parse(&req.name, labels).map(|m| m.labels))
        .transpose()
        .map_err(|why| CommandError::new(ErrorKind::Usage, why))?;
    let who = match cx.viewer() {
        Viewer::User(user) => format!("user {}", user.get()),
        Viewer::Operator | Viewer::Nobody => "operator".to_owned(),
    };
    let change = Change {
        max_boxes: req.max_boxes,
        labels: labels.as_deref(),
    };
    let wrote =
        cx.store()?
            .set_node_settings(&row, &req.name, &change, &who, req.expected_revision)?;
    match wrote {
        SettingsWrote::Written(written) => {
            if let Ok(node) = NodeName::parse(&req.name) {
                let _ = promote_detached(cx, &node);
            }
            Ok(value(&req.name, &written, true))
        }
        SettingsWrote::Stale { current } => {
            let now = current.unwrap_or_default();
            let wanted = req.expected_revision.unwrap_or_default();
            Err(CommandError::new(
                ErrorKind::Refused,
                format!(
                    "the settings of {} are at revision {now}, not {wanted}: they changed after you read them",
                    req.name
                ),
            )
            .with_hint("read them again with `moor node ls`, apply your change to what is there, and set it with that revision"))
        }
    }
}

/// The node `name` that the context's viewer may read, and whether it may change it.
fn find(cx: &Cx, name: &str) -> Result<Found, CommandError> {
    let store = cx.store()?;
    let machine = |row: NodeSettings, can_edit| Found::Machine { row, can_edit };
    let own = match cx.viewer() {
        Viewer::User(me) => store
            .node_settings(Some(me), name)?
            .map(|r| machine(r, true)),
        Viewer::Operator | Viewer::Nobody => None,
    };
    if let Some(found) = own {
        return Ok(found);
    }
    if let Some(row) = store.node_settings(None, name)? {
        return Ok(machine(row, cx.viewer() == Viewer::Operator));
    }
    configured(cx.config())
        .0
        .remove(name)
        .map(Found::Configured)
        .ok_or_else(|| {
            CommandError::new(ErrorKind::Refused, format!("you have no node named {name}"))
        })
}

/// The wire form of `row`, the settings of node `name`.
fn value(name: &str, row: &NodeSettings, can_edit: bool) -> NodeSettingsValue {
    NodeSettingsValue {
        name: name.to_owned(),
        scope: if row.owner.is_some() {
            NodeScope::Personal
        } else {
            NodeScope::Shared
        },
        max_boxes: row.max_boxes,
        labels: row.labels.clone(),
        revision: row.revision,
        edited_by: row.edited_by.clone(),
        edited_at: row.edited_at.map(rfc3339),
        can_edit,
    }
}

#[cfg(test)]
mod tests;
