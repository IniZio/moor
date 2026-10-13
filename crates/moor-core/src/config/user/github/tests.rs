//! Tests for the GitHub token command settings.

use crate::config::{parse_user, ConfigError, UserConfig};

use super::DEFAULT_YOU_TOKEN_CMD;

fn err(doc: &str) -> ConfigError {
    parse_user("u.yaml", doc).expect_err(doc)
}

fn ok(doc: &str) -> UserConfig {
    let r = parse_user("u.yaml", doc);
    assert!(r.is_ok(), "{doc}: {r:?}");
    r.unwrap_or_default()
}

#[test]
fn configured_argv_is_returned() {
    let c = ok("github: {you_token_cmd: [op, read, x]}");
    assert_eq!(c.github.token_cmd(), ["op", "read", "x"]);
}

#[test]
fn default_when_unset() {
    for doc in ["", "github: {}", "github: {pass_env: [GH_CONFIG_DIR]}"] {
        assert_eq!(ok(doc).github.token_cmd(), DEFAULT_YOU_TOKEN_CMD, "{doc:?}");
    }
    assert_eq!(DEFAULT_YOU_TOKEN_CMD, ["gh", "auth", "token"]);
}

#[test]
fn pass_env_parses_without_cmd() {
    let c = ok("github: {pass_env: [GH_CONFIG_DIR]}");
    assert_eq!(c.github.pass_env, ["GH_CONFIG_DIR"]);
}

#[test]
fn unknown_key_names_path() {
    let e = err("github: {token: x}");
    assert_eq!(e.path, "github.token");
    assert!(e.reason.contains("unknown field"), "{e:?}");
}

#[test]
fn empty_cmd_or_element_is_an_error() {
    for doc in [
        "github: {you_token_cmd: []}",
        "github: {you_token_cmd: ['']}",
    ] {
        assert_eq!(err(doc).path, "github.you_token_cmd", "{doc}");
    }
}

#[test]
fn pass_env_names_are_checked() {
    assert_eq!(
        err("github: {pass_env: [A, 1B]}").path,
        "github.pass_env[1]"
    );
    assert_eq!(err("github: {pass_env: [A, A]}").path, "github.pass_env[1]");
}

#[test]
fn scalar_cmd_is_a_parse_error() {
    let e = err("github: {you_token_cmd: \"gh auth token\"}");
    assert!(e.path.starts_with("github.you_token_cmd"), "{e:?}");
}
