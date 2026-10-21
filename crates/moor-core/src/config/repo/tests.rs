//! Repo config parse and validation tables.
#![allow(clippy::panic, clippy::indexing_slicing, reason = "test assertions")]

use super::{parse_repo, RepoConfig};
use crate::config::BoxSize;

fn ok(text: &str) -> RepoConfig {
    parse_repo("m.yaml", text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

/// Asserts the document is rejected with `path` and a message containing `needle`.
fn rejects(text: &str, path: &str, needle: &str) {
    match parse_repo("m.yaml", text) {
        Ok(_) => panic!("accepted: {text}"),
        Err(e) => {
            assert_eq!(e.file, "m.yaml", "{text}");
            assert_eq!(e.path, path, "{text}: {e}");
            assert!(e.to_string().contains(needle), "{text}: {e}");
        }
    }
}

#[test]
fn valueless_sections_are_defaults() {
    for key in [
        "box",
        "env",
        "setup",
        "teardown",
        "compose",
        "box_credentials",
    ] {
        for t in [format!("{key}:"), format!("{key}: ~")] {
            assert_eq!(ok(&t), RepoConfig::default(), "{t:?}");
        }
    }
}

#[test]
fn leading_zero_text_needs_quotes() {
    let c = ok("env: {A: \"0755\"}\nsetup: ['0755']");
    assert_eq!(c.env.get("A").map(String::as_str), Some("0755"));
    assert_eq!(c.setup, ["0755"]);
    rejects("env: {A: 0755}", "env.A", "must be quoted");
    rejects("setup: [0755]", "setup[0]", "must be quoted");
}

#[test]
fn binary_tag_keeps_the_text() {
    let c = ok("env: {A: !!binary aGVsbG8=}");
    assert_eq!(c.env.get("A").map(String::as_str), Some("aGVsbG8="));
}

#[test]
fn empty_documents_are_default() {
    for t in ["", "  \n", "null", "{}", "~"] {
        assert_eq!(ok(t), RepoConfig::default(), "{t:?}");
    }
}

#[test]
fn accepts_full_document() {
    let c = ok(r"
setup: ['true']
teardown: ['true']
env: {FOO: bar}
compose: {project_prefix: pp}
box: {size: large}
box_credentials: [npm-token, other-1]
");
    assert_eq!(c.box_.size, Some(BoxSize::Large));
    assert_eq!(c.compose.project_prefix.as_deref(), Some("pp"));
    assert_eq!(c.env["FOO"], "bar");
    assert_eq!(c.box_credentials, ["npm-token", "other-1"]);
}

#[test]
fn box_sizes() {
    for (s, want) in [
        ("small", BoxSize::Small),
        ("medium", BoxSize::Medium),
        ("large", BoxSize::Large),
        ("xlarge", BoxSize::Xlarge),
    ] {
        assert_eq!(ok(&format!("box: {{size: {s}}}")).box_.size, Some(want));
    }
    rejects("box: {size: huge}", "box.size", "huge");
}

#[test]
fn repo_box_only_allows_size() {
    for key in [
        "provider: sprites",
        "enforce_egress: true",
        "max_boxes: 2",
        "nope: 1",
    ] {
        let name = key.split(':').next().unwrap_or(key);
        rejects(
            &format!("box: {{{key}}}"),
            &format!("box.{name}"),
            "unknown field",
        );
    }
}

#[test]
fn removed_keys_are_unknown() {
    rejects("agent: {model: haiku}", "agent", "unknown field");
    rejects("agent: {location: box}", "agent", "unknown field");
    for t in [
        "reports: {backend: local}",
        "reports: {}",
        "record: {keep: [node_modules]}",
        "delivery: {mode: pr}",
        "egress: [x.io]",
        "sandbox: [android]",
        "sandbox: {}",
    ] {
        let key = t.split(':').next().unwrap_or(t);
        rejects(t, key, "unknown field");
    }
}

#[test]
fn unknown_keys_name_the_key() {
    rejects("egres: [a.example.com]", "egres", "egres");
    rejects("compose: {nope: 1}", "compose.nope", "compose.nope");
}

#[test]
fn trimmed_keys_are_unknown() {
    for key in ["credentials", "tools", "provision", "run", "git"] {
        rejects(&format!("{key}: []"), key, key);
    }
    rejects("git: {name: R}", "git", "git");
    rejects("run: {env: {CI: '1'}}", "run", "run");
}

#[test]
fn map_form_box_credentials_is_a_type_error() {
    rejects("box_credentials: {NPM_TOKEN: {}}", "box_credentials", "");
    rejects("box_credentials: {NPM_TOKEN: null}", "box_credentials", "");
}

#[test]
fn bad_yaml_has_invalid_yaml_reason() {
    rejects("{{{ not yaml", "", "invalid yaml");
}

#[test]
fn list_entries() {
    rejects("setup: [' ']", "setup[0]", "setup");
    rejects("setup: ['true', '']", "setup[1]", "setup");
    rejects("teardown: ['']", "teardown[0]", "teardown");
}

#[test]
fn env_entries() {
    rejects("env: {PATH: x}", "env.PATH", "PATH");
    rejects("env: {1BAD: x}", "env.1BAD", "1BAD");
}

#[test]
fn box_credentials_rejections() {
    rejects(
        "box_credentials: ['bad name']",
        "box_credentials[0]",
        "bad name",
    );
    rejects(
        "box_credentials: [npm-token, npm-token]",
        "box_credentials[1]",
        "twice",
    );
    for name in ["GH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN"] {
        rejects(
            &format!("box_credentials: [a, {name}]"),
            "box_credentials[1]",
            "secret name",
        );
    }
}

#[test]
fn changed_keys_empty_when_equal() {
    let a = ok("setup: [make]\nenv:\n  A: b\n");
    assert!(a.changed_keys(&a.clone()).is_empty());
}

#[test]
fn changed_keys_names_setup() {
    let a = ok("setup: [make]\n");
    let b = ok("setup: [make, test]\n");
    assert_eq!(a.changed_keys(&b), ["setup"]);
}

#[test]
fn changed_keys_lists_env_and_box_in_file_order() {
    let a = ok("env:\n  A: b\nbox:\n  size: small\n");
    let b = ok("env:\n  A: c\nbox:\n  size: large\n");
    assert_eq!(a.changed_keys(&b), ["env", "box"]);
}

#[test]
fn proxy_direct_lists_hosts_and_suffixes() {
    let cfg = ok("proxy: {direct: [dl.google.com, .Gradle.org]}");
    let forms: Vec<String> = cfg
        .proxy
        .direct
        .iter()
        .map(crate::name::DirectHost::no_proxy)
        .collect();
    assert_eq!(forms, ["dl.google.com", ".gradle.org"]);
}

#[test]
fn proxy_direct_refuses_a_host_a_credential_rule_serves() {
    for (entry, host) in [
        ("api.github.com", "api.github.com"),
        ("uploads.github.com", "uploads.github.com"),
        ("api.anthropic.com", "api.anthropic.com"),
        (".github.com", "api.github.com"),
        ("anthropic.com", "api.anthropic.com"),
    ] {
        rejects(
            &format!("proxy: {{direct: [pub.dev, {entry}]}}"),
            "proxy.direct[1]",
            &format!("covers {host},"),
        );
    }
}

#[test]
fn proxy_direct_refuses_wildcards_ips_and_schemes() {
    for entry in [
        "'*.gradle.org'",
        "10.0.0.1",
        "https://pub.dev",
        "localhost",
        "''",
    ] {
        rejects(
            &format!("proxy: {{direct: [{entry}]}}"),
            "proxy.direct[0]",
            "direct host must be a host name or .domain",
        );
    }
}

#[test]
fn a_repo_cannot_add_a_proxy_rule() {
    rejects(
        "proxy: {rules: {x: {hosts: [x.example], credential: forge}}}",
        "proxy.rules",
        "unknown field",
    );
}
