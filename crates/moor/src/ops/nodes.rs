//! `node-list`: the nodes the caller may start Boxes on.

use moor_core::decide::pressure::Reading;
use moor_core::decide::schedule::{Node, Online, Scope};
use moor_core::request::MachineLinksReq;
use moor_core::user::Viewer;
use moor_core::view::node::{NodeListValue, NodeScope, NodeView};
use moor_core::view::user::MachineLinksValue;
use moor_core::Warning;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::node::{configured, now};
use crate::ops::common::promote::promote_waiting;
use crate::ops::common::room::on_report;

/// Every node, shared and personal.
///
/// # Errors
/// A store failure.
pub(crate) fn all(cx: &Cx) -> Result<Vec<Node>, CommandError> {
    let (shared, _) = configured(cx.config());
    Ok(cx.store()?.nodes(&shared)?)
}

/// `machine-links`: records one runner's agent links, relieves each personal node under memory pressure,
/// then starts the Boxes pending on each personal node whose link is up.
///
/// # Errors
/// A store failure.
pub(crate) fn set_links(
    cx: &Cx,
    report: &MachineLinksReq,
) -> Result<MachineLinksValue, CommandError> {
    let store = cx.store()?;
    let links = store.set_machine_links(
        &report.runner,
        report.draining,
        &report.links,
        cx.clock().now(),
    )?;
    for node in all(cx)? {
        let Scope::Personal { machine_id, .. } = node.scope else {
            continue;
        };
        let Some(link) = links
            .iter()
            .find(|l| l.online && l.machine_id == machine_id)
        else {
            continue;
        };
        if let Some(m) = link.memory {
            let reading = Reading {
                at: m.at.as_millis() / 1000,
                available_mib: m.available_mib,
                psi_some_centi: m.psi_centi,
                psi_full_centi: m.psi_full_centi,
                disk_low: false,
            };
            if let Err(e) = on_report(cx, store, &node, reading) {
                cx.warn(Warning {
                    code: "pressure_failed".to_owned(),
                    message: format!(
                        "cannot relieve memory pressure on {}: {}",
                        node.name, e.error
                    ),
                });
            }
        }
        promote_waiting(cx, store, &node.name, "link");
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
fn view(
    node: &Node,
    now: i64,
    pressure_since: Option<i64>,
    disk_low: bool,
    box_memory_mib: Option<u64>,
) -> NodeView {
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
        mem_free_mib: node.memory.map(|m| m.free_mib()),
        disk_free_mib: node.memory.and_then(|m| m.disk_free_mib),
        disk_low,
        mem_psi_centi: node.memory.and_then(|m| m.psi_centi),
        mem_report_age_secs: node.memory.map(|m| now.saturating_sub(m.reported_at)),
        pressure_since,
        box_memory_mib,
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
    let since = cx.store()?.pressure_since()?;
    let held = cx.store()?.box_memory_totals()?;
    Ok(NodeListValue {
        nodes: nodes
            .iter()
            .map(|n| {
                let boxes = match n.scope {
                    Scope::Personal { machine_id, .. } => held.get(&machine_id).copied(),
                    Scope::Shared => None,
                };
                let low = cx
                    .store()
                    .and_then(|s| Ok(s.pressure(&n.name)?))
                    .is_ok_and(|p| p.readings.last().is_some_and(|r| r.disk_low));
                view(n, now(cx), since.get(n.name.as_str()).copied(), low, boxes)
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests;
