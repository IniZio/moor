use moor_core::ExitClass;

use super::*;

#[test]
fn unknown_tool_fails_without_spawning() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("ran");
    let value = serde_json::json!({
        "tool": "sh",
        "args": ["-c", format!("touch {}", marker.display())],
        "woke": false,
    });
    let env = Env::from_pairs([("PATH", std::env::var("PATH").unwrap_or_default())]);

    let resp = attach(&env, Response::ok("connect", Some(value)));

    assert_eq!(resp.class, ExitClass::Failed);
    assert_eq!(resp.cmd, "open");
    let message = resp.error.unwrap().message;
    assert!(message.contains("unknown tool `sh`"), "{message}");
    assert!(!marker.exists(), "unknown tool must not be spawned");
}
