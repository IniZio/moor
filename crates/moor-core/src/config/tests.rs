//! Tests for user config parsing.

use std::path::PathBuf;

use super::{parse_user, ConfigError, UserConfig};

#[test]
fn empty_doc_is_defaults() {
    assert_eq!(parse_user(""), Ok(UserConfig::default()));
    assert_eq!(parse_user("{}\n"), Ok(UserConfig::default()));
}

#[test]
fn session_defaults_to_moor() {
    let c = UserConfig::default();
    assert_eq!(c.herdr_session(), "moor");
    assert_eq!(c.provider(), None);
    assert_eq!(c.provider_or_default(), "local-container");
}

#[test]
fn provider_session_and_tools_parse() {
    let c = parse_user(
        "box: {provider: static, size: large, max_boxes: 3}\n\
         surface: {herdr: {session: work}}\n\
         tools: {crabbox: /opt/crabbox, git: git2}\n",
    )
    .unwrap_or_default();
    assert_eq!(c.provider(), Some("static"));
    assert_eq!(c.provider_or_default(), "static");
    assert_eq!(c.box_.size.as_deref(), Some("large"));
    assert_eq!(c.box_.max_boxes, Some(3));
    assert_eq!(c.herdr_session(), "work");
    assert_eq!(c.tools.crabbox, Some(PathBuf::from("/opt/crabbox")));
    assert_eq!(c.tools.git, Some(PathBuf::from("git2")));
    assert_eq!(c.tools.herdr, None);
}

#[test]
fn unknown_keys_are_ignored() {
    let c =
        parse_user("drivers: {crabbox: {ssh_port: 22}}\nfuture: 1\nbox: {provider: aws, x: 1}\n")
            .unwrap_or_default();
    assert_eq!(c.provider(), Some("aws"));
}

#[test]
fn malformed_yaml_is_config_error() {
    assert!(matches!(
        parse_user("box: [unclosed"),
        Err(ConfigError::Parse(_))
    ));
    assert!(matches!(
        parse_user("box: {max_boxes: many}"),
        Err(ConfigError::Parse(_))
    ));
}
