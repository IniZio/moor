//! The node tool against the fake controller.

use serde_json::json;

use super::mcp;

#[test]
fn list_nodes_sends_node_list_as_the_token_s_person_with_a_read_token() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let result = m.call(&token, "list_nodes", json!({}));
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(
        m.requests()[0]["request"],
        json!({"cmd": "as", "identity": "authgear:alice-sub", "request": {"cmd": "node-list"}})
    );
}

#[test]
fn the_tool_descriptions_send_the_agent_to_list_nodes_before_a_start() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let reply = m.rpc(&token, "tools/list", json!({}));
    let tools = reply["result"]["tools"].as_array().unwrap();
    let text = |name: &str| {
        tools
            .iter()
            .find(|t| t["name"] == name)
            .and_then(|t| t["description"].as_str())
            .unwrap()
            .to_owned()
    };
    assert!(text("list_nodes").contains("before start_box"));
    assert!(text("start_box").contains("names the node"));
}

/// Runs only with `MOOR_BIN` naming a built `moor`: a real controller with one shared node
/// answers `list_nodes` through the gate, and a start picks that node.
#[test]
#[ignore = "needs MOOR_BIN, a built moor binary"]
fn a_real_controller_lists_its_shared_node_and_the_pick() {
    let moor = std::env::var("MOOR_BIN").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".config/moor")).unwrap();
    std::fs::write(
        root.join(".config/moor/config.yaml"),
        "nodes: {ci: {runtime: static, max_boxes: 4}}\nbox: {node: ci}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("c.yaml"),
        format!(
            "socket: {}\nstate_dir: {}\nfronts:\n  authgear: {{}}\n",
            root.join("c.sock").display(),
            root.join("state").display()
        ),
    )
    .unwrap();
    let child = std::process::Command::new(moor)
        .args(["controller", "serve", "--config"])
        .arg(root.join("c.yaml"))
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join(".config"))
        .spawn()
        .unwrap();
    let mut controller = moor_testkit::ChildGuard::new(child);
    let socket = root.join("state/fronts/authgear/front");
    for _ in 0..200 {
        if socket.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let (_tx, rx) = std::sync::mpsc::channel();
    let gate_dir = tempfile::tempdir().unwrap();
    let m = super::Mcp {
        world: super::world_with(socket, rx, gate_dir, true),
        seen: std::sync::mpsc::channel().1,
    };
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let result = m.call(&token, "list_nodes", json!({}));
    controller.kill().unwrap();
    controller.wait().unwrap();
    assert_eq!(result["structuredContent"]["nodes"][0]["name"], "ci");
    assert_eq!(result["structuredContent"]["start"]["node"], "ci");
    assert_eq!(result["structuredContent"]["start"]["outcome"], "starts");
}
