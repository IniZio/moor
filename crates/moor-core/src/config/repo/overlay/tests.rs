//! The override's merge, as a table of (repo text, override text, what the Box sees).
#![allow(clippy::panic, clippy::unwrap_used, reason = "test assertions")]

use super::{with_env, RepoOverride};
use crate::config::{parse_repo, RepoConfig};

/// `over` laid on `base`, both parsed.
fn merged(base: &str, over: &str) -> RepoConfig {
    let base = parse_repo("base", base).unwrap_or_else(|e| panic!("{base}: {e}"));
    RepoOverride::parse("over", over)
        .unwrap_or_else(|e| panic!("{over}: {e}"))
        .over(base)
}

/// The merge of `base` and `over` equals the config `want` parses to.
fn is(base: &str, over: &str, want: &str) {
    let want = parse_repo("want", want).unwrap();
    assert_eq!(merged(base, over), want, "{base:?} + {over:?}");
}

#[test]
fn a_named_key_wins_and_an_unnamed_one_stays() {
    is(
        "brief: repo\nsetup: [a]\nteardown: [t]",
        "brief: box",
        "brief: box\nsetup: [a]\nteardown: [t]",
    );
    is("setup: [a]", "teardown: [x]", "setup: [a]\nteardown: [x]");
}

#[test]
fn env_merges_by_key_and_the_override_wins() {
    is(
        "env: {A: repo, C: repo}",
        "env: {A: box, B: box}",
        "env: {A: box, B: box, C: repo}",
    );
    is("env: {A: repo}", "env:", "env: {A: repo}");
    is("env: {A: repo}", "setup: [x]", "env: {A: repo}\nsetup: [x]");
}

#[test]
fn lists_replace_and_an_empty_list_clears() {
    is("setup: [a, b]", "setup: [c]", "setup: [c]");
    is("setup: [a, b]", "setup: []", "");
    is("teardown: [a]", "teardown: ~", "");
}

#[test]
fn sections_replace_whole() {
    is(
        "node: {labels: [gpu]}\nbox: {size: large}",
        "node: {labels: [arm]}",
        "node: {labels: [arm]}\nbox: {size: large}",
    );
    is("box: {size: large}", "box: {}", "");
    is(
        "compose: {project_prefix: p}",
        "compose: {project_prefix: q}",
        "compose: {project_prefix: q}",
    );
}

#[test]
fn an_override_over_nothing_is_the_override() {
    let text = "env: {A: box}\nsetup: [s]";
    let over = RepoOverride::parse("o", text).unwrap();
    assert_eq!(
        over.over(RepoConfig::default()),
        parse_repo("w", text).unwrap()
    );
}

#[test]
fn empty_text_changes_nothing() {
    for text in ["", "# nothing\n", "{}"] {
        let over = RepoOverride::parse("o", text).unwrap();
        assert!(over.is_empty(), "{text:?}");
        is("setup: [a]\nenv: {A: x}", text, "setup: [a]\nenv: {A: x}");
    }
}

#[test]
fn the_override_is_checked_as_a_repo_config() {
    for text in ["bogus: 1", "setup: ['']", "env: {PATH: x}", "env: [a]"] {
        assert!(RepoOverride::parse("o", text).is_err(), "{text}");
    }
}

#[test]
fn proxy_entries_add_to_the_repos_for_the_same_rule() {
    let a = "PATCH /repos/{repo}/issues/comments/*";
    let b = "POST /repos/{repo}/statuses/*";
    is(
        &format!("proxy: {{allow: {{github-api: [\"{a}\"]}}}}"),
        &format!("proxy: {{allow: {{github-api: [\"{a}\", \"{b}\"]}}}}"),
        &format!("proxy: {{allow: {{github-api: [\"{a}\", \"{b}\"]}}}}"),
    );
    is(
        "setup: [x]",
        &format!("proxy: {{allow: {{github-api: [\"{b}\"]}}}}"),
        &format!("setup: [x]\nproxy: {{allow: {{github-api: [\"{b}\"]}}}}"),
    );
    is(
        &format!("proxy: {{allow: {{github-api: [\"{a}\"]}}}}"),
        "setup: [x]",
        &format!("setup: [x]\nproxy: {{allow: {{github-api: [\"{a}\"]}}}}"),
    );
}

#[test]
fn an_override_with_a_bad_proxy_entry_does_not_parse() {
    let bad = "proxy: {allow: {github-api: [\"PATCH /repos/{any_repo}/x\"]}}";
    assert!(RepoOverride::parse("over", bad).is_err());
}

/// The variables a pull request run adds.
fn pull_vars() -> Vec<(&'static str, String)> {
    vec![
        ("MOOR_PULL_NUMBER", "65".to_owned()),
        ("MOOR_PULL_REPO", "o/r".to_owned()),
    ]
}

#[test]
fn env_added_to_an_override_keeps_its_other_keys_and_wins_a_clash() {
    let over = "setup: [make]\nbox: {size: large}\nenv: {MOOR_PULL_NUMBER: '1', KEEP: 'yes'}\n\
                proxy: {allow: {github-api: ['POST /repos/{repo}/issues/*/comments']}}";
    let text = with_env(Some(over), &pull_vars()).unwrap();
    is(
        "setup: [a]\nteardown: [t]\nenv: {REPO: x}",
        &text,
        "setup: [make]\nteardown: [t]\nbox: {size: large}\n\
         env: {REPO: x, KEEP: 'yes', MOOR_PULL_NUMBER: '65', MOOR_PULL_REPO: o/r}\n\
         proxy: {allow: {github-api: ['POST /repos/{repo}/issues/*/comments']}}",
    );
}

#[test]
fn env_added_to_no_override_names_only_env() {
    for none in [None, Some(""), Some("env:")] {
        let text = with_env(none, &pull_vars()).unwrap();
        is(
            "setup: [a]",
            &text,
            "setup: [a]\nenv: {MOOR_PULL_NUMBER: '65', MOOR_PULL_REPO: o/r}",
        );
    }
}

#[test]
fn env_is_not_added_to_an_override_the_repo_config_refuses() {
    assert!(with_env(Some("setp: [a]"), &pull_vars()).is_err());
}
