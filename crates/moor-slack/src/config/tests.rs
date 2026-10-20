//! Front config parse tables.
#![allow(clippy::panic, reason = "test assertions")]

use super::parse_config;
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
