//! Controller config parse tables.
#![allow(clippy::panic, reason = "test assertions")]

use super::{parse_controller, ControllerConfig};

fn err(text: &str) -> crate::config::ConfigError {
    match parse_controller("c.yaml", text) {
        Ok(c) => panic!("accepted {text}: {c:?}"),
        Err(e) => e,
    }
}

#[test]
fn empty_documents_are_default() {
    for text in ["", "  \n", "null", "{}"] {
        let c = parse_controller("c.yaml", text).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(c, ControllerConfig::default(), "{text}");
    }
}

#[test]
fn both_keys_parse() {
    let c = parse_controller("c.yaml", "socket: ~/run/c.sock\nstate_dir: /var/moor\n")
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(c.socket.as_deref(), Some("~/run/c.sock"));
    assert_eq!(c.state_dir.as_deref(), Some("/var/moor"));
}

#[test]
fn unknown_key_names_key_and_file() {
    let e = err("socket: /a\nbogus: 1\n");
    assert_eq!(e.file, "c.yaml");
    assert!(e.to_string().contains("bogus"), "{e}");
    assert!(e.to_string().contains("c.yaml"), "{e}");
}

#[test]
fn mistyped_value_names_key() {
    let e = err("socket: [a, b]\n");
    assert_eq!(e.path, "socket");
    assert_eq!(e.file, "c.yaml");
}

#[test]
fn empty_string_is_rejected() {
    for key in ["socket", "state_dir"] {
        let e = err(&format!("{key}: \"\"\n"));
        assert_eq!(e.path, key);
        assert_eq!(e.file, "c.yaml");
    }
}

#[test]
fn settle_interval_parses_and_defaults_to_five_seconds() {
    let c = parse_controller("c.yaml", "settle_every_secs: 30\n").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(c.settle_every(), std::time::Duration::from_secs(30));
    assert_eq!(
        ControllerConfig::default().settle_every(),
        std::time::Duration::from_secs(5)
    );
}

#[test]
fn drain_timeout_parses_and_defaults_to_ten_minutes() {
    let c = parse_controller("c.yaml", "drain_timeout_secs: 0\n").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(c.drain_timeout(), std::time::Duration::ZERO);
    assert_eq!(
        ControllerConfig::default().drain_timeout(),
        std::time::Duration::from_secs(600)
    );
}

#[test]
fn zero_settle_interval_names_key() {
    let e = err("settle_every_secs: 0\n");
    assert_eq!(e.path, "settle_every_secs");
    assert_eq!(e.file, "c.yaml");
}

#[test]
fn removed_proof_cap_key_is_unknown() {
    let e = err("proof_max_bytes: 1024\n");
    assert_eq!(e.path, "proof_max_bytes");
    assert!(e.reason.contains("unknown field"), "{e}");
}

#[test]
fn removed_web_key_points_to_moor_web() {
    for text in [
        "web: {port: 8765, token_file: x}\n",
        "web: ~\n",
        "web:\n",
        "web: null\n",
    ] {
        let e = err(text);
        assert_eq!(e.path, "web", "{text}");
        assert!(e.to_string().contains("moor-web"), "{text}: {e}");
    }
}

#[test]
fn removed_slack_key_says_to_delete_it() {
    for text in [
        "slack: {app_token_file: /x/app, bot_token_file: /x/bot, channels: [C1], users: [U1]}\n",
        "slack: ~\n",
        "slack:\n",
        "slack: null\n",
    ] {
        let e = err(text);
        assert_eq!(e.path, "slack", "{text}");
        let line = e.to_string();
        assert!(line.contains("delete it"), "{text}: {line}");
        assert!(line.contains("moor-slack"), "{text}: {line}");
        assert!(!line.contains('\n'), "{text}: {line}");
    }
}

#[test]
fn a_repos_key_is_an_unknown_key() {
    let e = err("repos:\n  app:\n    url: https://github.com/o/app.git\n");
    assert!(e.to_string().contains("unknown field `repos`"), "{e}");
}

#[test]
fn fronts_parse_by_namespace() {
    let c = parse_controller("c.yaml", "fronts:\n  slack: {}\n  web-2: {}\n")
        .unwrap_or_else(|e| panic!("{e}"));
    let names: Vec<&str> = c.fronts.keys().map(String::as_str).collect();
    assert_eq!(names, ["slack", "web-2"]);
}

#[test]
fn a_reserved_or_malformed_front_namespace_is_refused() {
    for ns in ["local", "Slack", "2x", "a_b"] {
        let e = err(&format!("fronts:\n  {ns}: {{}}\n"));
        assert_eq!(e.path, format!("fronts.{ns}"), "{e}");
        assert!(e.to_string().contains("not be `local`"), "{e}");
    }
    let e = err("fronts:\n  slack: {bogus: 1}\n");
    assert!(e.to_string().contains("bogus"), "{e}");
}
