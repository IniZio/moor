//! Tests for credential rows, which resolve against the secrets a user holds.

use std::collections::BTreeSet;

use moor_core::config::{parse_user, BoxSettings, Flags};
use moor_core::seam::forge::RepoFiles;
use moor_core::user::SecretName;

use super::{checked, unresolved};

/// Settings resolved from user config `yaml` alone.
fn settings(yaml: &str) -> BoxSettings {
    let user = parse_user("u.yaml", yaml).expect("user config");
    BoxSettings::resolve(&user, None, &Flags::default())
}

/// The secret names `names`, as a user holds them.
fn held(names: &[&str]) -> BTreeSet<SecretName> {
    names
        .iter()
        .map(|n| SecretName::parse(n).unwrap())
        .collect()
}

const SHARED: &str = "
box_credentials: [a, b]
mcp_servers:
  m: {url: 'https://x.example', headers: {Authorization: 'Bearer ${a}'}}
";

#[test]
fn rows_resolve_only_for_secrets_the_user_holds() {
    let failing = unresolved(&settings(SHARED), &RepoFiles::default(), &held(&["b"]));
    let names: Vec<&str> = failing.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["a"], "an MCP server missing its key never refuses the start");
    let a = failing.first().unwrap();
    assert_eq!(
        a.error.as_deref(),
        Some("log in first: moor user secret set a")
    );
    assert!(unresolved(&settings(SHARED), &RepoFiles::default(), &held(&["a", "b"])).is_empty());
}

#[test]
fn env_rows_leave_resolution_open() {
    let out = checked(&settings(SHARED), &RepoFiles::default());
    assert_eq!(out.credentials.len(), 2);
    assert!(out.credentials.iter().all(|i| i.resolves.is_none()));
    assert!(out.mcp.iter().all(|i| i.resolves.is_none()));
}

#[test]
fn a_box_credential_row_says_only_its_name_is_checked() {
    let out = checked(&settings(SHARED), &RepoFiles::default());
    for row in &out.credentials {
        assert_eq!(
            row.delivery.as_deref(),
            Some("name checked only"),
            "{}",
            row.name
        );
    }
}

#[test]
fn only_the_committed_claude_settings_approve_a_repo_server() {
    let files = |settings: &str| RepoFiles {
        mcp_json: Some(r#"{"mcpServers":{"a":{}}}"#.into()),
        claude_settings: Some(settings.into()),
        moor_yaml: None,
    };
    let approved = files(r#"{"enabledMcpjsonServers":["a"]}"#);
    assert!(unresolved(&settings(""), &approved, &held(&[])).is_empty());
    let other = files(r#"{"enabledMcpjsonServers":["b"]}"#);
    let failing = unresolved(&settings(""), &other, &held(&[]));
    let error = failing.first().and_then(|i| i.error.as_deref()).unwrap();
    assert!(
        error.contains(".claude/settings.json and push it"),
        "{error}"
    );
}
