use moor_core::ExitClass;

use super::*;

#[test]
fn unknown_tool_fails_without_spawning() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("ran");
    let value = serde_json::json!({
        "focus": {
            "tool": "sh",
            "args": ["-c", format!("touch {}", marker.display())],
        },
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

#[test]
fn no_focus_command_succeeds_without_spawning() {
    let env = Env::from_pairs([("PATH", std::env::var("PATH").unwrap_or_default())]);
    let value = serde_json::json!({"focus": null, "woke": true});

    let resp = attach(&env, Response::ok("connect", Some(value)));

    assert_eq!(resp.class, ExitClass::Ok);
    assert_eq!(resp.cmd, "open");
    assert!(resp.error.is_none());
}

#[test]
fn the_old_connect_shape_is_refused() {
    let env = Env::from_pairs([("PATH", std::env::var("PATH").unwrap_or_default())]);
    let value = serde_json::json!({"tool": "herdr", "args": [], "woke": false});

    let resp = attach(&env, Response::ok("connect", Some(value)));

    assert_eq!(resp.class, ExitClass::Failed);
}
