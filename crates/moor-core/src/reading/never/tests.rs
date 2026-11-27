#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::collections::BTreeMap;

use serde_json::json;

use crate::config::proxy::{default_rules, ToolRule};
use crate::graphql_scope::Scope;
use crate::reading::tests::{doc, reading};
use crate::reading::{
    Allowed, Effect, Found, IntentTable, Mark, Origin, Pinned, Read, Refusal, Request,
};

const SOURCE: &str = "Post the result. Never merge the pull request. Never push a branch.";
const MERGE: &str = "/repos/{repo}/pulls/*/merge";

fn never(exact: &str, intent: &str) -> serde_json::Value {
    json!({"quote": {"exact": exact}, "intents": [{"intent": intent, "polarity": "never"}]})
}

fn rest_merge() -> Request {
    Request::Http {
        host: "api.github.com".into(),
        method: "PUT".into(),
        path: MERGE.into(),
    }
}

fn merge_mutation() -> Request {
    Request::Mutation {
        host: "api.github.com".into(),
        name: "mergePullRequest".into(),
    }
}

fn read_under(
    phrases: &[serde_json::Value],
    rules: &BTreeMap<String, ToolRule>,
) -> Result<Read, Refusal> {
    let pinned = Pinned { rules, app: true };
    let v = json!({"phrases": phrases, "effects": []});
    reading(SOURCE).hand_in(&doc(v), IntentTable::shipped().unwrap(), &pinned)
}

/// The withheld effects of `read` as (request, origin).
fn withheld(read: &Read) -> Vec<(Request, Origin)> {
    read.effects()
        .iter()
        .filter_map(|e| match e {
            Effect::Withheld { request, from, .. } => Some((request.clone(), *from)),
            _ => None,
        })
        .collect()
}

/// The default rules with `entry` added to the rule for the GitHub API.
fn api_allows(entry: &str) -> BTreeMap<String, ToolRule> {
    let mut rules = default_rules();
    if let Some(api) = rules.get_mut("github-api") {
        api.allow.push(entry.to_owned());
    }
    rules
}

#[test]
fn never_merge_is_held_by_the_rest_merge_and_the_mutation_withheld() {
    let read = read_under(
        &[never("Never merge the pull request", "merge")],
        &default_rules(),
    )
    .unwrap();
    assert_eq!(
        withheld(&read),
        [
            (rest_merge(), Origin::Table),
            (merge_mutation(), Origin::Table)
        ]
    );
    assert!(
        matches!(&read.phrases()[0].mark, Mark::Held { effects } if effects.iter().count() == 2),
        "{:?}",
        read.phrases()[0].mark
    );
    assert_eq!(read.phrases()[0].intents[0].found, Found::Refused);
}

#[test]
fn never_push_is_asked_naming_the_rule_that_lets_pushes_through() {
    let read = read_under(&[never("Never push a branch", "push")], &default_rules()).unwrap();
    let allowed = Allowed {
        request: Request::Http {
            host: "github.com".into(),
            method: "POST".into(),
            path: "/{repo}.git/git-receive-pack".into(),
        },
        rule: "github-git".into(),
    };
    assert_eq!(
        read.phrases()[0].mark,
        Mark::Asked {
            allowed: Some(allowed.clone())
        }
    );
    assert_eq!(
        read.phrases()[0].intents[0].found,
        Found::LetThrough(allowed)
    );
    assert!(read.effects().is_empty());
}

#[test]
fn never_merge_under_rules_that_allow_the_merge_is_asked_naming_their_rule() {
    let rules = api_allows(&format!("PUT {MERGE}"));
    let read = read_under(&[never("Never merge the pull request", "merge")], &rules).unwrap();
    assert_eq!(
        read.phrases()[0].mark,
        Mark::Asked {
            allowed: Some(Allowed {
                request: rest_merge(),
                rule: "github-api".into()
            })
        }
    );
}

#[test]
fn a_rules_entry_that_lets_one_pull_request_merge_lets_never_merge_through() {
    let rules = api_allows("PUT /repos/{repo}/pulls/7/merge");
    let read = read_under(&[never("Never merge the pull request", "merge")], &rules).unwrap();
    assert!(matches!(
        read.phrases()[0].mark,
        Mark::Asked { allowed: Some(_) }
    ));
}

#[test]
fn a_scope_that_lists_the_merge_mutation_lets_never_merge_through() {
    let mut rules = default_rules();
    let scope: Scope = serde_json::from_value(
        json!({"query": ["repository"], "mutation": ["mergePullRequest(pullRequestId)"]}),
    )
    .unwrap();
    if let Some(api) = rules.get_mut("github-api") {
        api.graphql = Some(scope);
    }
    let read = read_under(&[never("Never merge the pull request", "merge")], &rules).unwrap();
    assert_eq!(
        read.phrases()[0].mark,
        Mark::Asked {
            allowed: Some(Allowed {
                request: merge_mutation(),
                rule: "github-api".into()
            })
        }
    );
}

#[test]
fn never_merge_beside_words_that_ask_for_the_merge_names_the_reading_own_line() {
    let ask = json!({"quote": {"exact": "Post the result"}, "intents": [{"intent": "merge", "polarity": "do"}]});
    let read = read_under(
        &[ask, never("Never merge the pull request", "merge")],
        &default_rules(),
    )
    .unwrap();
    assert!(matches!(read.phrases()[0].mark, Mark::Held { .. }));
    assert_eq!(
        read.phrases()[1].mark,
        Mark::Asked {
            allowed: Some(Allowed {
                request: rest_merge(),
                rule: "github-api".into()
            })
        }
    );
    assert!(withheld(&read).is_empty());
}

#[test]
fn two_phrases_forbidding_the_merge_hold_the_same_withheld_effects() {
    let read = read_under(
        &[
            never("Post the result", "merge"),
            never("Never merge the pull request", "merge"),
        ],
        &default_rules(),
    )
    .unwrap();
    assert_eq!(withheld(&read).len(), 2);
    assert_eq!(read.phrases()[0].mark, read.phrases()[1].mark);
}

#[test]
fn never_on_a_named_repo_withholds_that_repo_s_request() {
    let named = json!({"quote": {"exact": "Never merge the pull request"},
                       "intents": [{"intent": "merge", "polarity": "never", "repo": "acme/web"}]});
    let read = read_under(&[named], &default_rules()).unwrap();
    assert_eq!(
        withheld(&read)[0].0,
        Request::Http {
            host: "api.github.com".into(),
            method: "PUT".into(),
            path: "/repos/acme/web/pulls/*/merge".into(),
        }
    );
}

#[test]
fn a_withheld_effect_keys_by_its_host_and_request() {
    let read = read_under(
        &[never("Never merge the pull request", "merge")],
        &default_rules(),
    )
    .unwrap();
    let keys: Vec<String> = read.effects().iter().map(|e| e.key().to_string()).collect();
    assert_eq!(
        keys,
        [
            format!("withheld:api.github.com:PUT {MERGE}"),
            "withheld:api.github.com:POST /graphql mutation mergePullRequest".to_owned(),
        ]
    );
}

#[test]
fn a_stored_reading_from_before_forbidding_words_still_loads() {
    let stored = json!({
        "phrases": [{"id": "p1", "quote": {"exact": "Never push"}, "at": {"start": 0, "end": 10},
                     "mark": {"kind": "asked"}}],
        "effects": [],
        "reader": {"box_name": "acme/reader-1", "harness": "claude"}
    });
    let read: Read = serde_json::from_value(stored).unwrap();
    assert_eq!(read.phrases()[0].mark, Mark::Asked { allowed: None });
}

#[test]
fn a_stored_forbidding_reading_reads_back_the_same() {
    let read = read_under(
        &[
            never("Never merge the pull request", "merge"),
            never("Never push a branch", "push"),
        ],
        &default_rules(),
    )
    .unwrap();
    let back: Read = serde_json::from_value(serde_json::to_value(&read).unwrap()).unwrap();
    assert_eq!(back, read);
}
