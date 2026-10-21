//! Tests for credential rows, which resolve against the secrets a user holds.

use std::collections::BTreeSet;

use moor_core::config::{parse_user, BoxSettings, Flags};
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
  m: {url: 'https://x/${a}'}
";

#[test]
fn rows_resolve_only_for_secrets_the_user_holds() {
    let d = tempfile::tempdir().unwrap();
    let failing = unresolved(&settings(SHARED), d.path(), &held(&["b"]));
    let names: Vec<&str> = failing.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["a", "m"]);
    let a = failing.first().unwrap();
    assert_eq!(
        a.error.as_deref(),
        Some("log in first: moor user secret set a")
    );
    assert!(unresolved(&settings(SHARED), d.path(), &held(&["a", "b"])).is_empty());
}

#[test]
fn env_rows_leave_resolution_open() {
    let d = tempfile::tempdir().unwrap();
    let out = checked(&settings(SHARED), d.path());
    assert_eq!(out.credentials.len(), 2);
    assert!(out.credentials.iter().all(|i| i.resolves.is_none()));
    assert!(out.mcp.iter().all(|i| i.resolves.is_none()));
}

#[test]
fn a_box_credential_row_says_only_its_name_is_checked() {
    let d = tempfile::tempdir().unwrap();
    let out = checked(&settings(SHARED), d.path());
    for row in &out.credentials {
        assert_eq!(
            row.delivery.as_deref(),
            Some("name checked only"),
            "{}",
            row.name
        );
    }
}
