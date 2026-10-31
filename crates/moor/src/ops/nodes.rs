//! `node-list`: the nodes the caller may start Boxes on.

use std::collections::HashMap;

use moor_core::config::Flags;
use moor_core::decide::schedule::{Node, Online, Scope};
use moor_core::user::Viewer;
use moor_core::view::node::{NodeListValue, NodeScope, NodeView};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::facts::{provider_of, settings_of};

/// Pending Boxes per provider id, by the provider their settings ask for. A Box whose settings
/// no longer resolve waits on no node.
fn waiting(cx: &Cx) -> Result<HashMap<String, usize>, CommandError> {
    let mut out = HashMap::new();
    for row in cx.store()?.waiting_boxes()? {
        let Ok(cfg) = settings_of(cx, &row, &Flags::default()) else {
            continue;
        };
        if let Ok(provider) = provider_of(&cfg) {
            *out.entry(provider.to_owned()).or_default() += 1;
        }
    }
    Ok(out)
}

/// Every node, shared and personal.
///
/// # Errors
/// A store failure.
pub(crate) fn all(cx: &Cx) -> Result<Vec<Node>, CommandError> {
    Ok(cx.store()?.nodes(&cx.config().nodes, &waiting(cx)?)?)
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
