//! Controller config parse tables.
#![allow(clippy::panic, reason = "test assertions")]

use super::{find_repo, parse_controller, ControllerConfig, MoorYamlSource, RepoTable};

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
fn zero_settle_interval_names_key() {
    let e = err("settle_every_secs: 0\n");
    assert_eq!(e.path, "settle_every_secs");
    assert_eq!(e.file, "c.yaml");
}

#[test]
fn proof_cap_defaults_and_parses() {
    assert_eq!(ControllerConfig::default().proof_max_bytes(), 104_857_600);
    let c = parse_controller("c.yaml", "proof_max_bytes: 2048\n").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(c.proof_max_bytes(), 2048);
}

#[test]
fn zero_proof_cap_names_key() {
    let e = err("proof_max_bytes: 0\n");
    assert_eq!(e.path, "proof_max_bytes");
    assert_eq!(e.file, "c.yaml");
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

const SLACK: &str = "slack:
  app_token_file: /x/app
  bot_token_file: /x/bot
  channels: [C0123ABC]
  users: [U0123ABC, U9]
";

fn slack_with(extra: &str) -> String {
    format!("{SLACK}{extra}")
}

#[test]
fn slack_section_parses_with_default_api_base() {
    let c = parse_controller("c.yaml", &slack_with("")).unwrap_or_else(|e| panic!("{e}"));
    let s = c.slack.unwrap_or_else(|| panic!("slack missing"));
    assert_eq!(s.app_token_file, std::path::PathBuf::from("/x/app"));
    assert_eq!(s.bot_token_file, std::path::PathBuf::from("/x/bot"));
    assert_eq!(s.channels, ["C0123ABC"]);
    assert_eq!(s.users, ["U0123ABC", "U9"]);
    assert_eq!(s.api_base, "https://slack.com/api");
    let c = parse_controller("c.yaml", &slack_with("  api_base: http://127.0.0.1:9000\n"))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        c.slack.map(|s| s.api_base).as_deref(),
        Some("http://127.0.0.1:9000")
    );
}

#[test]
fn slack_absent_is_none() {
    let c = parse_controller("c.yaml", "socket: /a\n").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(c.slack, None);
}

#[test]
fn slack_empty_channels_or_users_are_rejected() {
    let e = err(&SLACK.replace("[C0123ABC]", "[]"));
    assert_eq!(e.path, "slack.channels");
    let e = err(&SLACK.replace("[U0123ABC, U9]", "[]"));
    assert_eq!(e.path, "slack.users");
    let e = err(&SLACK.replace("[U0123ABC, U9]", "[\"\"]"));
    assert_eq!(e.path, "slack.users");
}

#[test]
fn slack_api_base_must_be_https_or_loopback_without_trailing_slash() {
    for base in [
        "http://slack.com/api",
        "http://example.com:80",
        "ftp://x",
        "https://slack.com/api/",
    ] {
        let e = err(&slack_with(&format!("  api_base: {base}\n")));
        assert_eq!(e.path, "slack.api_base", "{base}");
    }
}

#[test]
fn slack_unknown_key_is_rejected() {
    let e = err(&slack_with("  bogus: 1\n"));
    assert!(e.to_string().contains("bogus"), "{e}");
}

const REPOS: &str = "repos:
  app:
    url: https://example.com/o/app.git
    default_branch: main
    moor_yaml: {path: ~/cfg/app.yaml}
  lib:
    url: git@example.com:o/lib/
    moor_yaml:
      inline: |
        box:
          size: small
";

fn repos() -> RepoTable {
    parse_controller("c.yaml", REPOS)
        .unwrap_or_else(|e| panic!("{e}"))
        .repos
}

#[test]
fn repos_block_parses_path_and_inline() {
    let r = repos();
    let (app, lib) = (r.get("app").expect("app"), r.get("lib").expect("lib"));
    assert_eq!(app.default_branch.as_deref(), Some("main"));
    assert_eq!(
        app.moor_yaml,
        Some(MoorYamlSource::Path("~/cfg/app.yaml".into()))
    );
    assert!(matches!(&lib.moor_yaml, Some(MoorYamlSource::Inline(t)) if t.contains("small")));
    assert_eq!(lib.default_branch, None);
}

#[test]
fn repo_entry_unknown_key_names_path() {
    let e = err("repos:\n  app:\n    url: u\n    bogus: 1\n");
    assert!(e.to_string().contains("bogus"), "{e}");
    assert!(e.to_string().contains("repos.app"), "{e}");
}

#[test]
fn bad_repo_entries_are_rejected() {
    for (text, path) in [
        ("repos:\n  ../x:\n    url: u\n", "repos.../x"),
        ("repos:\n  '':\n    url: u\n", "repos."),
        ("repos:\n  a:\n    url: ' '\n", "repos.a.url"),
        (
            "repos:\n  a:\n    url: u\n    default_branch: -x\n",
            "repos.a.default_branch",
        ),
        (
            "repos:\n  a:\n    url: u\n    default_branch: ''\n",
            "repos.a.default_branch",
        ),
        (
            "repos:\n  a:\n    url: u\n    moor_yaml: {path: ''}\n",
            "repos.a.moor_yaml",
        ),
        (
            "repos:\n  a:\n    url: u\n    moor_yaml: {inline: 'bogus: 1'}\n",
            "repos.a.moor_yaml",
        ),
    ] {
        let e = err(text);
        assert!(e.to_string().contains(path), "{text}: {e}");
    }
}

#[test]
fn url_credentials_are_rejected_without_echo() {
    for url in [
        "https://user:s3cr3ttok@github.com/o/r",
        "https://s3cr3ttok@github.com/o/r",
        "user:s3cr3ttok@github.com:o/r.git",
    ] {
        let e = err(&format!("repos:\n  a:\n    url: '{url}'\n"));
        let msg = e.to_string();
        assert!(msg.contains("repos.a.url"), "{url}: {msg}");
        assert!(!msg.contains("s3cr3t"), "{url}: {msg}");
    }
}

#[test]
fn credential_free_urls_are_accepted() {
    for url in [
        "git@github.com:o/r.git",
        "ssh://git@host/o/r.git",
        "https://github.com/o/r",
    ] {
        let text = format!("repos:\n  a:\n    url: '{url}'\n");
        parse_controller("c.yaml", &text).unwrap_or_else(|e| panic!("{url}: {e}"));
    }
}

#[test]
fn find_repo_by_name_and_url() {
    let r = repos();
    assert_eq!(find_repo(&r, "app").map(|(n, _)| n), Some("app"));
    for key in [
        "https://example.com/o/app.git",
        "https://example.com/o/app",
        "https://example.com/o/app/",
        "https://example.com/o/app.git/",
    ] {
        assert_eq!(find_repo(&r, key).map(|(n, _)| n), Some("app"), "{key}");
    }
    assert_eq!(
        find_repo(&r, "git@example.com:o/lib.git").map(|(n, _)| n),
        Some("lib")
    );
    assert!(find_repo(&r, "nope").is_none());
    assert!(find_repo(&r, "https://example.com/o/other").is_none());
}
