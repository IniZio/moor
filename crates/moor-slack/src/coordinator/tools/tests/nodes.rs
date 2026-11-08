//! `list_nodes`: the node listing the coordinator reads before it starts a Box.
#![allow(clippy::panic, clippy::indexing_slicing, reason = "test code")]

use moor_core::request::Request;
use moor_core::response::Response;
use moor_core::view::node::{NodeListValue, NodeScope, NodeView, PickOutcome, StartPick};
use serde_json::json;

use super::{two_authors, Controller};
use crate::coordinator::tools::{names, Call};

fn mini() -> NodeView {
    NodeView {
        name: "mini".into(),
        scope: NodeScope::Personal,
        owner: None,
        runtime: "cloudhypervisor".into(),
        online: true,
        offline_since: None,
        used: 1,
        waiting: 0,
        max_boxes: Some(4),
        mem_free_mib: Some(8192),
        disk_free_mib: None,
        disk_low: false,
        mem_psi_centi: None,
        mem_report_age_secs: None,
        pressure_since: None,
        warm_starts: None,
        box_memory_mib: None,
        labels: Vec::new(),
    }
}

#[test]
fn list_nodes_sends_node_list_and_tells_the_agent_which_node_a_start_picks() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_authors(dir.path());
    let c = Controller::new();
    let value = NodeListValue {
        at: 0,
        nodes: vec![mini()],
        start: Some(StartPick {
            node: Some("mini".into()),
            outcome: PickOutcome::Starts,
            reason: None,
        }),
    };
    let reply = c.run_with(
        &db,
        &Call::parse("list_nodes", &json!({})).unwrap(),
        &mut |r| {
            Ok(Response::ok(
                r.cmd(),
                Some(serde_json::to_value(&value).unwrap()),
            ))
        },
    );
    assert!(!reply.is_error, "{}", reply.text);
    assert!(matches!(c.sent.borrow()[0], Request::NodeList(_)));
    assert!(reply
        .text
        .contains("mini (yours, cloudhypervisor) online, 1 of 4 Boxes, 8 GiB free"));
    assert!(reply.text.contains("goes to mini"));
}

#[test]
fn list_nodes_is_in_the_tool_list_and_says_to_call_it_before_start_box() {
    assert!(names().contains(&"list_nodes"));
    let spec = crate::coordinator::tools::SPECS
        .iter()
        .find(|s| s.name == "list_nodes")
        .unwrap();
    assert!(spec.description.contains("before start_box"));
}
