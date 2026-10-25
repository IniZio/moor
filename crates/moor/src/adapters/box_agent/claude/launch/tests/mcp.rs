use std::collections::BTreeMap;

use moor_core::config::McpServer;

use super::*;

fn linear() -> McpServer {
    McpServer {
        url: Some("https://mcp.example.test/mcp".to_owned()),
        headers: BTreeMap::from([("Authorization".to_owned(), "Bearer moor-proxy".to_owned())]),
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
    let c = with(dir.path(), linear());
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
