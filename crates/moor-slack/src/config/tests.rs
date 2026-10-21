//! Front config parse tables.
#![allow(clippy::panic, reason = "test assertions")]

use super::{parse_config, CoordinatorAgent};
use moor_core::config::ConfigError;

const SLACK: &str = "app_token_file: /x/app
bot_token_file: /x/bot
channels: [C0123ABC]
users: [U0123ABC, U9]
";

fn slack_with(extra: &str) -> String {
    format!("{SLACK}{extra}")
}

fn err(text: &str) -> ConfigError {
    match parse_config("s.yaml", text) {
        Ok(c) => panic!("accepted {text}: {c:?}"),
        Err(e) => e,
    }
}

#[test]
fn slack_section_parses_with_default_api_base() {
    let s = parse_config("s.yaml", &slack_with("")).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(s.app_token_file, std::path::PathBuf::from("/x/app"));
    assert_eq!(s.bot_token_file, std::path::PathBuf::from("/x/bot"));
    assert_eq!(s.channels, ["C0123ABC"]);
    assert_eq!(s.users, ["U0123ABC", "U9"]);
    assert_eq!(s.api_base, "https://slack.com/api");
    let s = parse_config("s.yaml", &slack_with("api_base: http://127.0.0.1:9000\n"))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(s.api_base, "http://127.0.0.1:9000");
}

#[test]
fn slack_empty_channels_or_users_are_rejected() {
    let e = err(&SLACK.replace("[C0123ABC]", "[]"));
    assert_eq!(e.path, "channels");
    let e = err(&SLACK.replace("[U0123ABC, U9]", "[]"));
    assert_eq!(e.path, "users");
    let e = err(&SLACK.replace("[U0123ABC, U9]", "[\"\"]"));
    assert_eq!(e.path, "users");
}

#[test]
fn slack_api_base_must_be_https_or_loopback_without_trailing_slash() {
    for base in [
        "http://slack.com/api",
        "http://example.com:80",
        "ftp://x",
        "https://slack.com/api/",
    ] {
        let e = err(&slack_with(&format!("api_base: {base}\n")));
        assert_eq!(e.path, "api_base", "{base}");
    }
}

#[test]
fn slack_unknown_key_is_rejected() {
    let e = err(&slack_with("bogus: 1\n"));
    assert!(e.to_string().contains("bogus"), "{e}");
}

#[test]
fn coordinator_binary_is_optional_and_must_be_absolute() {
    let s = parse_config("s.yaml", &slack_with("")).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(s.coordinator.binary, None);
    let text = slack_with("coordinator:\n  binary: /opt/claude/bin/claude\n");
    let s = parse_config("s.yaml", &text).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        s.coordinator.binary,
        Some(std::path::PathBuf::from("/opt/claude/bin/claude"))
    );
    let e = err(&slack_with("coordinator:\n  binary: claude\n"));
    assert_eq!(e.path, "coordinator.binary");
    let e = err(&slack_with("coordinator:\n  binary: /c\n  budget: 1\n"));
    assert!(e.to_string().contains("budget"), "{e}");
}

#[test]
fn coordinator_defaults_to_claude_on_sonnet_with_32_kib_and_one_open_box() {
    for text in [slack_with(""), slack_with("coordinator:\n  binary: /c\n")] {
        let c = parse_config("s.yaml", &text).unwrap().coordinator;
        assert_eq!(c.agent, CoordinatorAgent::Claude);
        let defaults = (c.model.as_str(), c.context_kib, c.max_open_boxes);
        assert_eq!(defaults, ("sonnet", 32, 1), "{text}");
    }
    let text = slack_with(
        "coordinator:\n  agent: claude\n  binary: /c\n  model: opus\n  context_kib: 8\n  \
         max_open_boxes: 2\n",
    );
    let c = parse_config("s.yaml", &text).unwrap().coordinator;
    assert_eq!(
        (c.model.as_str(), c.context_kib, c.max_open_boxes),
        ("opus", 8, 2)
    );
    let e = err(&slack_with("coordinator:\n  max_open_boxes: 0\n"));
    assert_eq!(e.path, "coordinator.max_open_boxes");
}

#[test]
fn a_coordinator_agent_other_than_claude_is_refused() {
    for agent in ["codex", "Claude", "\"\""] {
        let e = err(&slack_with(&format!(
            "coordinator:\n  agent: {agent}\n  binary: /c\n"
        )));
        assert!(e.to_string().contains("agent"), "{agent}: {e}");
    }
    let e = err(&slack_with(
        "coordinator:\n  binary: /c\n  context_kib: 0\n",
    ));
    assert_eq!(e.path, "coordinator.context_kib");
}
