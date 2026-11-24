//! Over the sockets, `box-tool` answers only a front whose config sets `box_tools: true`, asserting
//! no identity: the front names the agent place from its transport.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded frames"
)]

use serde_json::json;

use super::fronts::{ask, refused, Served};

#[test]
fn only_a_box_tools_front_asserting_no_identity_reaches_the_place_check() {
    let served = Served::start_fronts(
        tempfile::tempdir().expect("dir"),
        "fronts:\n  gate:\n    box_tools: true\n  web: {}\n",
        &[],
    );
    let call = json!({
        "cmd": "box-tool",
        "from": 7,
        "call": {"tool": "exec", "target": "w", "command": "ls", "timeout": 30}
    });
    refused(&ask(&served.front("gate"), &call), "Box 7 is not open");
    refused(&ask(&served.front("web"), &call), "`box_tools: true`");
    refused(&ask(&served.main, &call), "`box_tools: true`");
    let forged = json!({"cmd": "as", "identity": "gate:someone", "request": call});
    refused(&ask(&served.front("gate"), &forged), "asserts no identity");
}
