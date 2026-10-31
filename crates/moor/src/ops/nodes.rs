//! `node-list`: the nodes the caller may start Boxes on.

use moor_core::decide::schedule::{Node, Online, Scope};
use moor_core::request::MachineLink;
use moor_core::user::Viewer;
use moor_core::view::node::{NodeListValue, NodeScope, NodeView};
use moor_core::view::user::MachineLinksValue;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::node::configured;
use crate::ops::common::promote::promote_waiting;

/// Every node, shared and personal.
///
/// # Errors
/// A store failure.
pub(crate) fn all(cx: &Cx) -> Result<Vec<Node>, CommandError> {
    let (shared, _) = configured(cx.config());
    Ok(cx.store()?.nodes(&shared)?)
}

/// `machine-links`: records the agent links, then starts the Boxes pending on each personal node
/// whose link is up.
///
/// # Errors
/// A store failure.
pub(crate) fn set_links(cx: &Cx, links: &[MachineLink]) -> Result<MachineLinksValue, CommandError> {
    let store = cx.store()?;
    let links = store.set_machine_links(links)?;
    for node in all(cx)? {
        let Scope::Personal { machine_id, .. } = node.scope else {
            continue;
        };
        if links.iter().any(|l| l.online && l.machine_id == machine_id) {
            promote_waiting(cx, store, &node.name, "link");
        }
    }
    Ok(MachineLinksValue { links })
}

/// The nodes `viewer` sees: shared ones, plus its own personal ones; the operator sees all.
fn visible(nodes: Vec<Node>, viewer: Viewer) -> Vec<Node> {
    nodes
        .into_iter()
        .filter(|n| match (&n.scope, viewer) {
            (_, Viewer::Operator) | (Scope::Shared, _) => true,
            (Scope::Personal { owner, .. }, Viewer::User(me)) => *owner == me,
            (Scope::Personal { .. }, Viewer::Nobody) => false,
        })
        .collect()
}

/// The wire form of `node`.
fn view(node: &Node) -> NodeView {
    let (scope, owner) = match &node.scope {
        Scope::Shared => (NodeScope::Shared, None),
        Scope::Personal { owner, .. } => (NodeScope::Personal, Some(*owner)),
    };
    let (online, offline_since) = match node.online {
        Online::Online => (true, None),
        Online::Offline { since } => (false, Some(since)),
    };
    NodeView {
        name: node.name.to_string(),
        scope,
        owner,
        runtime: node.runtime.clone(),
        online,
        offline_since,
        used: node.used,
        waiting: node.waiting,
        max_boxes: node.max_boxes,
        labels: node.labels.clone(),
    }
}

/// `node-list`: the nodes the context's viewer sees, shared ones first.
///
/// # Errors
/// A store failure.
pub(crate) fn list(cx: &Cx) -> Result<NodeListValue, CommandError> {
    let mut nodes = visible(all(cx)?, cx.viewer());
    nodes.sort_by_key(|n| (matches!(n.scope, Scope::Personal { .. }), n.name.clone()));
    Ok(NodeListValue {
        nodes: nodes.iter().map(view).collect(),
    })
}

#[cfg(test)]
mod tests;
