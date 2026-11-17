//! `node ls` names how a machine provides Boxes when its agent says.

use moor_core::Response;
use serde_json::json;

use crate::cli::render_text::render;

#[test]
fn node_ls_shows_how_the_agent_provides_boxes_in_place_of_the_runtime() {
    let v = json!({"nodes": [
        {"name": "local", "scope": "shared", "runtime": "boxes", "provides": "docker",
         "online": true, "used": 1, "waiting": 0, "max_boxes": 4, "labels": []},
        {"name": "mini", "scope": "personal", "owner": 3, "runtime": "cloudhypervisor",
         "online": true, "used": 0, "waiting": 0, "labels": []},
    ]});
    let (out, _) = render(&Response::ok("node-list", Some(v)), false);
    assert_eq!(
        out,
        "local  shared  docker  online  1/4\nmini  personal  cloudhypervisor  online  0 (no cap)\n"
    );
}
