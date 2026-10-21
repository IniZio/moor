//! The built-in direct list and its documentation.
#![allow(clippy::panic, reason = "test assertions")]

use super::*;
use crate::config::proxy::default_rules;

#[test]
fn every_builtin_host_is_a_valid_direct_entry() {
    let parsed: Vec<String> = builtin_direct(&[])
        .iter()
        .map(DirectHost::no_proxy)
        .collect();
    assert_eq!(parsed, BUILTIN_DIRECT);
}

#[test]
fn no_builtin_host_covers_a_default_rule_host() {
    let rules = default_rules();
    let hosts: Vec<&str> = rules
        .values()
        .flat_map(|r| r.hosts.iter().map(String::as_str))
        .collect();
    assert!(!hosts.is_empty());
    assert_eq!(builtin_direct(&hosts).len(), BUILTIN_DIRECT.len());
    for host in ["ghcr.io", "github.com", "gitlab.com"] {
        assert_eq!(
            builtin_direct(&[host]).len(),
            BUILTIN_DIRECT.len(),
            "{host}"
        );
    }
}

/// The text of `docs/<name>` at the repo root.
fn doc(name: &str) -> String {
    let path = format!("{}/../../docs/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The `## Direct hosts` section of docs/providers.md.
fn direct_hosts_section() -> String {
    let text = doc("providers.md");
    let (_, rest) = text
        .split_once("\n## Direct hosts\n")
        .unwrap_or_else(|| panic!("no Direct hosts section"));
    rest.split("\n## ").next().unwrap_or_default().to_owned()
}

#[test]
fn the_docs_list_every_builtin_host_and_the_opt_out() {
    let section = direct_hosts_section();
    for host in BUILTIN_DIRECT {
        assert!(
            section.contains(&format!("`{host}`")),
            "providers.md lacks {host}"
        );
    }
    assert!(section.contains("default_direct: false"), "{section}");
    let config = doc("config.md");
    assert!(
        config.contains("| `proxy.default_direct` |"),
        "config.md lacks proxy.default_direct"
    );
}
