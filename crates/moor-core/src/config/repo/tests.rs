//! Repo config parse and validation tables.
#![allow(clippy::panic, clippy::indexing_slicing, reason = "test assertions")]

use super::{parse_repo, RepoConfig, SeedMode};
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
egress: [Example.COM, '*.example.org']
env: {FOO: bar}
compose: {project_prefix: pp}
box: {size: large}
box_credentials: [NPM_TOKEN, other-1]
seed: [.env]
run: {env: {CI: '1'}, path: [bin]}
");
    assert_eq!(c.seed.len(), 1);
    assert_eq!(c.run.path, ["bin"]);
    assert_eq!(c.box_.size, Some(BoxSize::Large));
    assert_eq!(c.compose.project_prefix.as_deref(), Some("pp"));
    assert_eq!(c.env["FOO"], "bar");
    assert_eq!(c.box_credentials, ["NPM_TOKEN", "other-1"]);
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
    for key in ["credentials", "tools", "provision", "git"] {
        rejects(&format!("{key}: []"), key, key);
    }
    rejects("git: {name: R}", "git", "git");
}

#[test]
fn seed_accepts_both_forms() {
    let c = ok("seed: [.env, {path: data, mode: symlink}, {path: x}]");
    let got: Vec<_> = c.seed.iter().map(|s| (s.path.to_str(), s.mode)).collect();
    assert_eq!(
        got,
        [
            (Some(".env"), SeedMode::Copy),
            (Some("data"), SeedMode::Symlink),
            (Some("x"), SeedMode::Copy),
        ]
    );
}

#[test]
fn seed_map_errors_name_the_entry() {
    rejects("seed: [{path: a, mode: hardlink}]", "seed[0]", "");
    rejects("seed: [ok, {path: a, nope: 1}]", "seed[1]", "");
    rejects("seed: [{mode: copy}]", "seed[0]", "");
}

#[test]
fn seed_paths_stay_inside_the_repo() {
    rejects("seed: [/etc/passwd]", "seed[0]", "relative");
    rejects("seed: [../x]", "seed[0]", "relative");
    rejects("seed: [{path: a/../../b}]", "seed[0]", "relative");
    rejects("seed: [ok, /abs]", "seed[1]", "relative");
    rejects("seed: ['']", "seed[0]", "relative");
    rejects("seed: [{path: '', mode: symlink}]", "seed[0]", "relative");
    rejects("seed: [.]", "seed[0]", "relative");
    rejects("seed: ['./']", "seed[0]", "relative");
    rejects(
        "seed: [ok, {path: ., mode: symlink}]",
        "seed[1]",
        "relative",
    );
    assert_eq!(ok("seed: ['./foo']").seed.len(), 1);
}

#[test]
fn seed_type_errors_do_not_leak_internal_names() {
    rejects(
        "seed: [1]",
        "seed[0]",
        "a path string or a {path, mode} map",
    );
}

#[test]
fn run_section() {
    let c = ok("run: {env: {CI: '1'}, path: [/opt/bin]}");
    assert_eq!(c.run.env["CI"], "1");
    assert_eq!(c.run.path, ["/opt/bin"]);
    rejects("run: {nope: 1}", "run.nope", "unknown field");
}

#[test]
fn run_env_entries() {
    rejects("run: {env: {PATH: x}}", "run.env.PATH", "PATH");
    rejects("run: {env: {1BAD: x}}", "run.env.1BAD", "1BAD");
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
fn egress_entries() {
    for bad in [
        "10.0.0.1",
        "localhost",
        "bad_char.com",
        "https://x.com",
        "''",
    ] {
        rejects(&format!("egress: [{bad}]"), "egress[0]", "egress");
    }
    rejects(
        "egress: [a.example.com, localhost]",
        "egress[1]",
        "localhost",
    );
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
        "box_credentials: [NPM_TOKEN, NPM_TOKEN]",
        "box_credentials[1]",
        "twice",
    );
    rejects(
        "box_credentials: [GH_TOKEN]",
        "box_credentials[0]",
        "GH_TOKEN",
    );
    rejects(
        "box_credentials: [A, GH_TOKEN]",
        "box_credentials[1]",
        "GH_TOKEN",
    );
}

#[test]
fn box_credentials_proxied_claude_names() {
    for name in ["CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_API_KEY"] {
        rejects(
            &format!("box_credentials: [{name}]"),
            "box_credentials[0]",
            "moor proxy",
        );
    }
}
