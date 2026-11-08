//! The nodes check.

use moor_core::view::node::{NodeListValue, NodeScope, NodeView};

use super::*;

/// A shared node as `node-list` shows it, fresh and roomy.
fn node(name: &str) -> NodeView {
    NodeView {
        name: name.to_owned(),
        scope: NodeScope::Shared,
        owner: None,
        runtime: "cloudhypervisor".to_owned(),
        online: true,
        offline_since: None,
        used: 0,
        waiting: 0,
        max_boxes: Some(4),
        mem_free_mib: Some(16 * 1024),
        disk_free_mib: None,
        disk_low: false,
        mem_psi_centi: Some(0),
        mem_report_age_secs: Some(5),
        pressure_since: None,
        warm_starts: None,
        box_memory_mib: None,
        labels: Vec::new(),
    }
}

/// A rig whose controller lists `nodes`.
fn listing(nodes: Vec<NodeView>) -> Rig {
    Rig::new().with_controller(Box::new(move |r| {
        answered(
            r.cmd(),
            serde_json::to_value(NodeListValue {
                nodes: nodes.clone(),
                at: 0,
                start: None,
            })
            .unwrap(),
        )
    }))
}

fn check_of(rig: &Rig) -> Check {
    rig.with(|i| nodes::check(i, &nodes::read(i)))
}

#[test]
fn fails_with_no_node_and_points_at_the_setup_step() {
    let c = check_of(&Rig::new());
    assert_eq!(c.status, Status::Fail);
    assert_eq!(c.detail, "no node configured");
    assert!(c.fix.contains("/nonexistent/moor.yaml") && c.fix.contains("docs/run.md step 3"));
}

#[test]
fn a_fresh_roomy_node_is_ok() {
    let c = check_of(&listing(vec![node("local")]));
    assert_eq!(c.status, Status::Ok);
    assert_eq!(c.detail, "local: report 5s old; room for a Box");
}

#[test]
fn a_report_older_than_a_minute_warns() {
    let mut n = node("local");
    n.mem_report_age_secs = Some(61);
    let c = check_of(&listing(vec![n]));
    assert_eq!(c.status, Status::Warn);
    assert!(
        c.detail.contains("no recent memory report from local"),
        "{c:?}"
    );
}

#[test]
fn a_node_short_of_memory_warns_with_the_scheduler_reason() {
    let mut n = node("local");
    n.mem_free_mib = Some(1024);
    let c = check_of(&listing(vec![n]));
    assert_eq!(c.status, Status::Warn);
    assert!(
        c.detail
            .contains("waiting for capacity on local: 1 GiB free"),
        "{c:?}"
    );
}

#[test]
fn memory_pressure_a_full_node_and_an_offline_node_warn() {
    let mut pressed = node("a");
    pressed.mem_psi_centi = Some(1500);
    let mut full = node("b");
    full.used = 4;
    let mut down = node("c");
    down.online = false;
    for n in [pressed, full, down] {
        assert_eq!(check_of(&listing(vec![n])).status, Status::Warn);
    }
}

#[test]
fn one_roomy_node_among_blocked_ones_still_warns_naming_the_blocked() {
    let mut full = node("b");
    full.used = 4;
    let c = check_of(&listing(vec![node("a"), full]));
    assert!(
        c.detail.contains("b is full") && !c.detail.contains("a is"),
        "{c:?}"
    );
}

#[test]
fn configured_nodes_without_a_controller_are_listed_without_reports() {
    let rig = Rig::new().with_config("nodes:\n  local:\n    runtime: cloudhypervisor\n");
    let c = check_of(&rig);
    assert_eq!(c.status, Status::Ok);
    assert!(c.detail.contains("1 configured"), "{c:?}");
}

#[test]
fn a_controller_that_cannot_list_nodes_fails() {
    let rig =
        Rig::new().with_controller(Box::new(|_| Err(ClientError::Protocol("boom".to_owned()))));
    let c = check_of(&rig);
    assert_eq!(c.status, Status::Fail);
    assert!(c.detail.contains("cannot list nodes"), "{c:?}");
}
