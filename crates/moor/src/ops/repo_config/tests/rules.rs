//! A repo's Proxy rules text: read seeded, written checked, and followed by the next plan.

use std::collections::BTreeMap;

use moor_core::config::proxy::DEFAULT_RULES_TEXT;
use moor_core::request::{RepoConfigGetReq, RepoConfigSetReq, RepoFile};
use moor_core::ErrorKind;

use super::{forge, operator, repo, BASE};
use crate::ops::common::proxy::plan;
use crate::ops::repo_config::{get, set};
use crate::testing::forge::repo_at;

fn get_rules() -> RepoConfigGetReq {
    RepoConfigGetReq {
        identity: None,
        repo: repo(),
        file: RepoFile::Rules,
    }
}

fn set_rules(text: &str, expected_revision: Option<u64>) -> RepoConfigSetReq {
    RepoConfigSetReq {
        identity: None,
        repo: repo(),
        file: RepoFile::Rules,
        text: text.to_owned(),
        expected_revision,
    }
}

/// The `METHOD path` entries the plan of a Box of the repo gives `api.github.com`.
fn github_entries(cx: &crate::cx::Cx) -> Vec<String> {
    let p = plan(cx, &[repo()], &BTreeMap::new(), false, &BTreeMap::new()).unwrap();
    p.rules
        .iter()
        .filter(|r| r.hosts == ["api.github.com"])
        .flat_map(|r| {
            r.endpoint_rules
                .iter()
                .map(|e| format!("{} {}", e.method, e.path))
        })
        .collect()
}

#[test]
fn a_repo_with_no_rules_reads_the_default_text_at_revision_0() {
    let dir = tempfile::tempdir().unwrap();
    let cx = operator(dir.path(), &forge(Ok(repo_at(BASE))));
    let read = get(&cx, &get_rules()).unwrap();
    assert_eq!(read.text.as_deref(), Some(DEFAULT_RULES_TEXT));
    assert_eq!(read.revision, 0);
    let config = get(&cx, &super::get_req()).unwrap();
    assert_eq!(config.text, None, "the config stays its own text");
}

#[test]
fn rules_that_do_not_parse_are_refused_with_their_place() {
    let dir = tempfile::tempdir().unwrap();
    let cx = operator(dir.path(), &forge(Ok(repo_at(BASE))));
    let bad =
        "rules:\n  - hosts: [a.example.com]\n    credential: NOPE\n    inject: {basic_user: u}\n";
    let e = set(&cx, &set_rules(bad, Some(0))).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Usage);
    assert!(
        e.error.to_string().contains("rules[0].credential"),
        "{}",
        e.error
    );
    assert_eq!(get(&cx, &get_rules()).unwrap().revision, 0);
}

#[test]
fn the_next_plan_follows_a_saved_edit() {
    let dir = tempfile::tempdir().unwrap();
    let cx = operator(dir.path(), &forge(Ok(repo_at(BASE))));
    assert!(github_entries(&cx).contains(&"GET /user".to_owned()));
    let edited = DEFAULT_RULES_TEXT.replace("      - \"GET /user\"\n", "");
    assert_ne!(edited, DEFAULT_RULES_TEXT);
    let wrote = set(&cx, &set_rules(&edited, Some(0))).unwrap();
    assert_eq!(wrote.revision, 1);
    assert!(!github_entries(&cx).contains(&"GET /user".to_owned()));
    let back = set(&cx, &set_rules(DEFAULT_RULES_TEXT, Some(1))).unwrap();
    assert_eq!(back.revision, 2);
    assert!(github_entries(&cx).contains(&"GET /user".to_owned()));
}

#[test]
fn a_grant_on_a_rule_the_repo_dropped_is_refused_plainly() {
    let dir = tempfile::tempdir().unwrap();
    let cx = operator(dir.path(), &forge(Ok(repo_at(BASE))));
    let only_git = "rules:
  - hosts: [github.com]
    credential: github
    inject: {basic_user: x-access-token}
";
    set(&cx, &set_rules(only_git, Some(0))).unwrap();
    let grant = BTreeMap::from([(
        "github-api".to_owned(),
        vec!["PATCH /repos/{repo}/issues/comments/*".to_owned()],
    )]);
    let Err(why) = plan(&cx, &[repo()], &BTreeMap::new(), false, &grant) else {
        unreachable!("planned")
    };
    assert!(why.contains("o/n's rules have no such rule"), "{why}");
}
