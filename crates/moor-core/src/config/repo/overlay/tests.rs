//! The override's merge, as a table of (repo text, override text, what the Box sees).
#![allow(clippy::panic, clippy::unwrap_used, reason = "test assertions")]

use super::RepoOverride;
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
