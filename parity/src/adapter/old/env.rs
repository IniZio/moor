use std::path::PathBuf;

use crate::world::World;

pub fn seam_env(w: &World) -> Vec<(String, String)> {
    let config = w.root().join("agent-config.json");
    if !config.exists() {
        let doc = serde_json::json!({
            "projects": { w.root().to_string_lossy(): { "hasTrustDialogAccepted": true } }
        });
        std::fs::write(&config, doc.to_string()).expect("write agent-config.json");
    }
    let path = |p: PathBuf| p.to_string_lossy().into_owned();
    let pairs = [
        ("MOOR_CRABBOX", path(w.tool_path("crabbox"))),
        ("MOOR_HERDR_BIN", path(w.tool_path("herdr"))),
        ("MOOR_AGENT_BIN", path(w.tool_path("claude"))),
        ("MOOR_AGENT_CONFIG", path(config)),
        (
            "MOOR_AGENT_DAEMON_LOG",
            path(w.home().join(".claude/daemon.log")),
        ),
        ("MOOR_WAIT_POLL_MS", "50".to_string()),
        ("MOOR_AGENT_PROMPT_WINDOW_MS", "0".to_string()),
        ("MOOR_AGENT_SETTLE_TIMEOUT_MS", "5000".to_string()),
        ("MOOR_GITHUB_API", "http://127.0.0.1:9".to_string()),
        ("MOOR_GITHUB_WEB", "http://127.0.0.1:9".to_string()),
    ];
    let preset = w.base_env();
    pairs
        .into_iter()
        .filter(|(k, _)| !k.starts_with("MOOR_GITHUB_") || !preset.iter().any(|(p, _)| p == k))
        .map(|(k, v)| (k.to_string(), v))
        .collect()
}
