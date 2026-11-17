//! `node set` says what it left.

use moor_core::Response;
use serde_json::json;

use super::render;

#[test]
fn node_set_says_the_cap_labels_and_revision_left() {
    let say = |max: serde_json::Value, labels: serde_json::Value| {
        let v = json!({"name": "mini", "scope": "personal", "max_boxes": max, "labels": labels,
            "revision": 2, "can_edit": true});
        render(&Response::ok("node-settings-set", Some(v)), false).0
    };
    assert_eq!(
        say(json!(3), json!(["gpu", "big"])),
        "mini: at most 3 Boxes, labels gpu,big (revision 2)\n"
    );
    assert_eq!(
        say(json!(null), json!([])),
        "mini: no cap, no labels (revision 2)\n"
    );
}
