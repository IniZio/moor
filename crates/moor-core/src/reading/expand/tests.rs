#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::collections::BTreeMap;

use serde_json::json;

use crate::config::proxy::{default_rules, ToolRule};
use crate::reading::tests::{asked, doc, held, read, reading};
use crate::reading::{
    Effect, Found, IntentTable, Line, Mark, Origin, Pinned, Read, Refusal, Unholdable,
};

const SOURCE: &str = "Take screenshots of each page. Post one comment on the pull request.";
const POST: &str = "POST /repos/{repo}/issues/*/comments";
const PATCH: &str = "PATCH /repos/{repo}/issues/comments/*";
const GET: &str = "GET /repos/{repo}/issues/*/comments";

fn intent(exact: &str, holds: &[&str], intent: &serde_json::Value) -> serde_json::Value {
    json!({"quote": {"exact": exact}, "holds": holds, "intents": [intent]})
}

fn comment() -> serde_json::Value {
    json!({"intent": "comment", "polarity": "do"})
}

fn shots() -> serde_json::Value {
    json!({"kind": "gate", "label": "shots", "gate": {"name": "screenshots", "evidence": ["shot-*"]}})
}

fn line(rule: &str, entry: &str) -> Line {
    Line {
        rule: rule.into(),
        entry: entry.into(),
    }
}

/// The allow effects of `read` as (rule, entry, origin).
fn allows(read: &Read) -> Vec<(String, String, Origin)> {
    read.effects()
        .iter()
        .filter_map(|e| match e {
            Effect::Allow {
                rule, entry, from, ..
            } => Some((rule.clone(), entry.clone(), *from)),
            _ => None,
        })
        .collect()
}

fn held_ids(read: &Read, at: usize) -> Vec<String> {
    let Mark::Held { effects } = &read.phrases()[at].mark else {
        return Vec::new();
    };
    effects.iter().map(ToString::to_string).collect()
}

/// `v` handed in for `SOURCE` under `rules`, with or without an App.
fn read_under(
    v: serde_json::Value,
    rules: &BTreeMap<String, ToolRule>,
    app: bool,
) -> Result<Read, Refusal> {
    let pinned = Pinned { rules, app };
    reading(SOURCE).hand_in(&doc(v), IntentTable::shipped().unwrap(), &pinned)
}

#[test]
fn comment_expands_to_its_row_less_the_line_the_rules_already_allow() {
    let read = read(
        SOURCE,
        json!({"phrases": [held("Take screenshots", &["shots"]), intent("Post one comment", &[], &comment())],
               "effects": [shots()]}),
    )
    .unwrap();
    assert_eq!(
        allows(&read),
        [
            ("github-api".into(), POST.into(), Origin::Table),
            ("github-api".into(), PATCH.into(), Origin::Table),
        ]
    );
    assert_eq!(held_ids(&read, 1), ["e2", "e3"]);
    assert_eq!(
        read.phrases()[1].intents[0].found,
        Found::Lines {
            already: vec![line("github-api", GET)]
        }
    );
}

#[test]
fn a_phrase_holding_a_gate_and_an_intent_holds_both() {
    let read = read(
        SOURCE,
        json!({"phrases": [intent("Post one comment", &["shots"], &comment())], "effects": [shots()]}),
    )
    .unwrap();
    assert_eq!(held_ids(&read, 0), ["e1", "e2", "e3"]);
}

#[test]
fn an_intent_no_row_has_marks_its_phrase_unmapped_and_holds_nothing() {
    let read = read(
        SOURCE,
        json!({"phrases": [intent("Take screenshots", &[], &json!({"intent": "upload-shots", "polarity": "do"}))], "effects": []}),
    )
    .unwrap();
    assert_eq!(read.phrases()[0].mark, Mark::Unmapped {});
    assert_eq!(read.phrases()[0].intents[0].found, Found::NoRow);
    assert!(read.effects().is_empty());
}

#[test]
fn a_gate_named_by_a_phrase_that_holds_nothing_is_refused_by_label() {
    let got = read(
        SOURCE,
        json!({"phrases": [intent("Take screenshots", &["shots"], &json!({"intent": "upload-shots", "polarity": "do"}))],
               "effects": [shots()]}),
    );
    assert_eq!(
        got.unwrap_err(),
        Refusal::Unheld {
            label: "shots".into()
        }
    );
}

#[test]
fn two_phrases_with_one_intent_hold_the_same_lines() {
    let read = read(
        SOURCE,
        json!({"phrases": [intent("Take screenshots", &[], &comment()), intent("Post one comment", &[], &comment())], "effects": []}),
    )
    .unwrap();
    assert_eq!(allows(&read).len(), 2);
    assert_eq!(held_ids(&read, 0), held_ids(&read, 1));
}

#[test]
fn as_app_sends_the_api_lines_under_the_app() {
    let app = json!({"intent": "comment", "polarity": "do", "as": "app"});
    let read = read(
        SOURCE,
        json!({"phrases": [intent("Post one comment", &[], &app)], "effects": []}),
    )
    .unwrap();
    let rules: Vec<_> = allows(&read).into_iter().map(|(r, e, _)| (r, e)).collect();
    assert_eq!(
        rules,
        [
            ("github-app".into(), POST.into()),
            ("github-app".into(), PATCH.into()),
            ("github-app".into(), GET.into())
        ]
    );
}

#[test]
fn as_app_on_a_controller_with_no_app_holds_nothing_and_says_why() {
    let app = json!({"intent": "comment", "polarity": "do", "as": "app"});
    let read = read_under(
        json!({"phrases": [intent("Post one comment", &[], &app)], "effects": []}),
        &default_rules(),
        false,
    )
    .unwrap();
    assert_eq!(read.phrases()[0].mark, Mark::Asked {});
    assert!(matches!(
        &read.phrases()[0].intents[0].found,
        Found::CannotHold {
            why: Unholdable::NoApp,
            ..
        }
    ));
    assert!(read.effects().is_empty());
}

#[test]
fn a_line_for_a_host_the_rules_lack_holds_nothing() {
    let mut rules = default_rules();
    rules.remove("github-api");
    let read = read_under(
        json!({"phrases": [intent("Post one comment", &[], &comment())], "effects": []}),
        &rules,
        true,
    )
    .unwrap();
    assert_eq!(read.phrases()[0].mark, Mark::Asked {});
    assert!(matches!(
        &read.phrases()[0].intents[0].found,
        Found::CannotHold {
            why: Unholdable::NoRuleForHost,
            ..
        }
    ));
}

#[test]
fn a_named_repo_is_written_in_place_of_the_box_repo() {
    let named = json!({"intent": "comment", "polarity": "do", "repo": "acme/web"});
    let read = read(
        SOURCE,
        json!({"phrases": [intent("Post one comment", &[], &named)], "effects": []}),
    )
    .unwrap();
    assert_eq!(allows(&read)[0].1, "POST /repos/acme/web/issues/*/comments");
}

#[test]
fn a_repo_the_box_lacks_is_refused_by_name() {
    let other = json!({"intent": "comment", "polarity": "do", "repo": "acme/api"});
    let got = read(
        SOURCE,
        json!({"phrases": [intent("Post one comment", &[], &other)], "effects": []}),
    );
    assert_eq!(
        got.unwrap_err(),
        Refusal::UnknownRepo {
            quote: "Post one comment".into(),
            repo: "acme/api".into()
        }
    );
}

#[test]
fn an_intent_name_that_is_not_lower_case_words_is_refused() {
    let bad = json!({"intent": "Comment", "polarity": "do"});
    let got = read(
        SOURCE,
        json!({"phrases": [intent("Post one comment", &[], &bad)], "effects": []}),
    );
    assert!(matches!(got.unwrap_err(), Refusal::IntentName { .. }));
}

#[test]
fn forbidding_words_hold_nothing_before_withheld_effects_exist() {
    let never = json!({"intent": "comment", "polarity": "never"});
    let read = read(SOURCE, json!({"phrases": [intent("Post one comment", &[], &never), asked("Take screenshots")], "effects": []})).unwrap();
    assert!(read.effects().is_empty());
    assert_eq!(read.phrases()[1].mark, Mark::Unmapped {});
}

#[test]
fn a_stored_slice_one_allow_reads_as_listed_and_a_table_one_keeps_its_origin() {
    let read = read(
        SOURCE,
        json!({"phrases": [intent("Post one comment", &[], &comment())], "effects": []}),
    )
    .unwrap();
    let text = serde_json::to_string(&read).unwrap();
    assert!(text.contains(r#""from":"table""#), "{text}");
    assert_eq!(serde_json::from_str::<Read>(&text).unwrap(), read);
    let old = json!({"kind": "allow", "id": "e1", "rule": "github-api", "entry": POST});
    let parsed: Effect = serde_json::from_value(old).unwrap();
    assert!(matches!(
        parsed,
        Effect::Allow {
            from: Origin::Listed,
            ..
        }
    ));
}
