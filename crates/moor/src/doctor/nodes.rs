//! The nodes check: one is configured, reports memory recently, and has room for a Box.

use moor_core::decide::schedule::{Shortfall, BOOT_FLOOR_MIB, PSI_LIMIT_CENTI, REPORT_FRESH_SECS};
use moor_core::name::NodeName;
use moor_core::request::AsReq;
use moor_core::view::node::{NodeListValue, NodeScope, NodeView};
use moor_core::Request;

use super::{answer, Check, Inputs, Problem};

/// The nodes the doctor looked at.
pub(super) struct Read {
    /// The controller's nodes, or the config's when no controller is set.
    pub nodes: Vec<NodeView>,
    /// Why the controller could not list them.
    pub problem: Option<Problem>,
    /// Whether the controller listed them, so memory reports are real.
    pub live: bool,
}

/// The nodes from the controller, else from the user config.
pub(super) fn read(i: &Inputs<'_>) -> Read {
    let mut problem = None;
    if i.target.is_some() {
        match answer::<NodeListValue>(i.ask, Request::NodeList(AsReq { identity: None })) {
            Ok(list) => {
                return Read {
                    nodes: list.nodes,
                    problem: None,
                    live: true,
                }
            }
            Err(p) => problem = Some(p),
        }
    }
    let nodes = i
        .config
        .nodes
        .iter()
        .map(|(name, n)| NodeView {
            name: name.clone(),
            scope: NodeScope::Shared,
            owner: None,
            runtime: n.runtime.clone(),
            online: true,
            offline_since: None,
            used: 0,
            waiting: 0,
            max_boxes: n.max_boxes.map(|m| m.get() as usize),
            mem_free_mib: None,
            disk_free_mib: None,
            disk_low: false,
            mem_psi_centi: None,
            mem_report_age_secs: None,
            pressure_since: None,
            box_memory_mib: None,
            labels: n.labels.clone(),
        })
        .collect();
    Read {
        nodes,
        problem,
        live: false,
    }
}

/// Why `node` cannot take a small Box now; none when it can.
fn blocker(node: &NodeView) -> Option<String> {
    let name = NodeName::parse(&node.name).ok()?;
    if !node.online {
        return Some(format!("{} is offline", node.name));
    }
    if node.max_boxes.is_some_and(|max| node.used >= max) {
        return Some(format!("{} is full", node.name));
    }
    let age = node.mem_report_age_secs?;
    let short = if age > REPORT_FRESH_SECS {
        Shortfall::Stale
    } else if node.mem_psi_centi.is_some_and(|p| p >= PSI_LIMIT_CENTI) {
        Shortfall::Pressure
    } else {
        let free_mib = node.mem_free_mib?;
        if free_mib >= BOOT_FLOOR_MIB {
            return None;
        }
        Shortfall::Low {
            free_mib,
            needs_mib: BOOT_FLOOR_MIB,
        }
    };
    Some(short.reason(&name))
}

/// What the report of `node` says, for the line.
fn report(node: &NodeView) -> String {
    node.mem_report_age_secs.map_or_else(
        || format!("{}: no memory report", node.name),
        |age| format!("{}: report {age}s old", node.name),
    )
}

/// At least one node, with fresh memory reports and room for a Box.
pub(super) fn check(i: &Inputs<'_>, read: &Read) -> Check {
    if let Some(p) = &read.problem {
        return Check::failed(
            "nodes",
            Problem {
                detail: format!("cannot list nodes: {}", p.detail),
                fix: p.fix.clone(),
            },
        );
    }
    if read.nodes.is_empty() {
        let at = crate::config_load::user_path(i.env)
            .map_or_else(|| "the user config".to_owned(), |p| p.display().to_string());
        return Check::fail(
            "nodes",
            "no node configured",
            format!("add one under nodes in {at} (docs/run.md step 3)"),
        );
    }
    let blocked: Vec<String> = read.nodes.iter().filter_map(blocker).collect();
    let reports: Vec<String> = read.nodes.iter().map(report).collect();
    let detail = reports.join("; ");
    if !blocked.is_empty() {
        return Check::warn(
            "nodes",
            format!("{detail}; no room now: {}", blocked.join(", ")),
            "wait for room, free memory on the node, or raise max_boxes",
        );
    }
    if !read.live {
        return Check::ok(
            "nodes",
            format!(
                "{} configured; memory reports come from a controller",
                read.nodes.len()
            ),
        );
    }
    Check::ok("nodes", format!("{detail}; room for a Box"))
}
