use super::{err, ok};

#[test]
fn nodes_without_a_provider_load() {
    let c = ok("nodes: {ci: {runtime: external}}");
    assert_eq!(c.default_node(), Some("ci"));
}

#[test]
fn removed_box_and_driver_limits_name_their_replacement() {
    for (doc, path, replacement) in [
        (
            "box: {provider: x}",
            "box.provider",
            "nodes: {name: {runtime: <provider>}}",
        ),
        ("box: {max_boxes: 2}", "box.max_boxes", "max_boxes: N"),
        (
            "drivers: {x: {max_boxes: 2}}",
            "drivers.x.max_boxes",
            "runtime: x, max_boxes: N",
        ),
    ] {
        let e = err(doc);
        assert_eq!(e.path, path, "{doc}");
        assert!(e.reason.contains("was removed"), "{e}");
        assert!(e.reason.contains(replacement), "{doc}: {e}");
    }
}

#[test]
fn a_node_runtime_may_not_name_a_host() {
    let e = err("nodes: {ci: {runtime: 'cloudhypervisor@hv1'}}");
    assert_eq!(e.path, "nodes.ci.runtime");
}
