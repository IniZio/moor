use std::collections::BTreeMap;
use std::process::Command;

use moor_core::config::McpServer;

use super::*;

fn linear(allow_tools: Option<Vec<String>>) -> McpServer {
    McpServer {
        url: Some("https://mcp.example.test/mcp".to_owned()),
        headers: BTreeMap::from([("Authorization".to_owned(), "Bearer moor-proxy".to_owned())]),
        allow_tools,
        ..McpServer::default()
    }
}

fn with(dir: &Path, server: McpServer) -> LaunchCtx {
    LaunchCtx {
        mcp: BTreeMap::from([("tracker".to_owned(), server)]),
        ..proxied(dir)
    }
}

fn mcp_config(argv: &[String]) -> Value {
    let at = argv
        .iter()
        .position(|a| a == "--mcp-config")
        .expect("--mcp-config");
    serde_json::from_str(&argv[at + 1]).unwrap()
}

#[test]
fn argv_hands_the_box_servers_to_the_agent() {
    let dir = TempDir::new().unwrap();
    let c = with(dir.path(), linear(None));
    for argv in [Claude.argv(&c), Claude.resume_argv("s1", &c)] {
        assert_eq!(
            mcp_config(&argv)["mcpServers"]["tracker"],
            json!({
                "type": "http",
                "url": "https://mcp.example.test/mcp",
                "headers": { "Authorization": "Bearer moor-proxy" },
            })
        );
    }
    assert!(!Claude
        .argv(&proxied(dir.path()))
        .contains(&"--mcp-config".to_owned()));
}

fn pre_tool_use(c: &LaunchCtx) -> Vec<Value> {
    let setup = Claude.hook_writer(c).unwrap();
    let fragment: Value = serde_json::from_str(&setup.files[1].content).unwrap();
    fragment["hooks"]["PreToolUse"].as_array().unwrap().clone()
}

/// Whether the agent's hook refuses `tool`: a matching entry's command exits 2.
fn refused(entries: &[Value], tool: &str) -> bool {
    entries.iter().any(|e| {
        let Some(matcher) = e["matcher"].as_str() else {
            return false;
        };
        let script = "process.exit(new RegExp(process.argv[1]).test(process.argv[2]) ? 0 : 1)";
        let hit = Command::new("node")
            .args(["-e", script, matcher, tool])
            .status()
            .unwrap()
            .success();
        let command = e["hooks"][0]["command"].as_str().unwrap();
        hit && Command::new("sh")
            .args(["-c", command])
            .status()
            .unwrap()
            .code()
            == Some(2)
    })
}

#[test]
fn a_tool_outside_allow_tools_is_refused() {
    if on_host_path("node").is_none() {
        eprintln!("skip: node not on PATH");
        return;
    }
    let dir = TempDir::new().unwrap();
    let c = with(
        dir.path(),
        linear(Some(strings(&["get_issue", "list_issues"]))),
    );
    let entries = pre_tool_use(&c);
    assert!(!refused(&entries, "mcp__tracker__get_issue"));
    assert!(!refused(&entries, "mcp__tracker__list_issues"));
    assert!(refused(&entries, "mcp__tracker__create_issue"));
    assert!(refused(&entries, "mcp__tracker__get_issue_x"));
    assert!(!refused(&entries, "Bash"));

    let none = with(dir.path(), linear(Some(Vec::new())));
    assert!(refused(&pre_tool_use(&none), "mcp__tracker__get_issue"));
    let all = with(dir.path(), linear(None));
    assert!(!refused(&pre_tool_use(&all), "mcp__tracker__create_issue"));
}
