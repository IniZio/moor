#![allow(clippy::indexing_slicing, reason = "test assertions")]
//! `node-list` values across releases.

use serde_json::json;

use super::{NodeListValue, NodeScope, NodeSettingsValue};

#[test]
fn a_node_from_a_release_before_node_settings_reads_as_unwritten_and_not_editable() {
    let old = json!({"nodes": [{"name": "mini", "scope": "personal", "runtime": "boxes",
        "online": true, "used": 1, "waiting": 0, "max_boxes": 4, "labels": []}], "at": 5});
    let list: NodeListValue = serde_json::from_value(old).unwrap();
    let node = list.nodes.first().unwrap();
    assert_eq!((node.revision, node.editable), (0, false));
    let wire = serde_json::to_value(&list).unwrap();
    assert!(wire["nodes"][0].get("revision").is_none(), "{wire}");
    assert!(wire["nodes"][0].get("editable").is_none(), "{wire}");
}

#[test]
fn settings_with_no_cap_leave_max_boxes_off_the_wire() {
    let value = NodeSettingsValue {
        name: "mini".to_owned(),
        scope: NodeScope::Personal,
        max_boxes: None,
        labels: vec![],
        revision: 0,
        edited_by: None,
        edited_at: None,
        can_edit: true,
    };
    let wire = serde_json::to_value(&value).unwrap();
    assert_eq!(
        wire,
        json!({"name": "mini", "scope": "personal", "labels": [], "revision": 0, "can_edit": true})
    );
}
