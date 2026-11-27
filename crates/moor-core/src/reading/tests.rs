#![allow(clippy::indexing_slicing, reason = "test assertions")]

use serde_json::json;

use super::{
    Context, HandIn, IntentTable, Pinned, Read, ReaderRun, Reading, ReadingId, ReadingOf,
    ReadingState, Refusal, RulesPin,
};
use crate::config::proxy::default_rules;
use crate::name::BoxName;
use crate::seam::clock::Ts;
use crate::user::UserId;
use crate::workflow::{PromptHash, WorkflowId};

pub(super) fn reader() -> ReaderRun {
    ReaderRun {
        box_name: BoxName::parse("acme/reader-1").unwrap(),
        harness: "claude".into(),
        model: Some("opus".into()),
    }
}

pub(super) fn reading(source: &str) -> Reading {
    Reading {
        id: ReadingId::parse("01936a3e-7c2b-7d4e-8f00-0123456789ab").unwrap(),
        of: ReadingOf::Automation(WorkflowId::new(7)),
        parent: None,
        source: source.into(),
        context: Context {
            source_hash: PromptHash::of(source),
            repos: vec![crate::name::ForgeRepo::new("acme", "web")],
            rules: vec![RulesPin {
                repo: None,
                revision: 3,
            }],
            instructions: 1,
            table: None,
        },
        made_by: UserId::new(1),
        state: ReadingState::Reading {
            by: reader(),
            since: Ts::from_millis(1),
        },
    }
}

pub(super) fn doc(v: serde_json::Value) -> HandIn {
    serde_json::from_value(v).unwrap()
}

/// `doc` handed in against the shipped table and the default rules, on a controller with an App.
pub(super) fn hand_in(r: &Reading, doc: &HandIn) -> Result<Read, Refusal> {
    let rules = default_rules();
    let pinned = Pinned {
        rules: &rules,
        app: true,
    };
    r.hand_in(doc, IntentTable::shipped().unwrap(), &pinned)
}

pub(super) fn read(source: &str, v: serde_json::Value) -> Result<Read, Refusal> {
    hand_in(&reading(source), &doc(v))
}

pub(super) fn asked(exact: &str) -> serde_json::Value {
    json!({"quote": {"exact": exact}})
}

fn placed(source: &str, phrases: &[serde_json::Value]) -> Vec<String> {
    let read = read(source, json!({"phrases": phrases, "effects": []})).unwrap();
    read.runs(source)
        .unwrap()
        .into_iter()
        .filter(|(_, p)| p.is_some())
        .map(|(text, _)| text.to_owned())
        .collect()
}

#[test]
fn a_quote_found_twice_without_a_prefix_is_refused_by_name() {
    let got = read(
        "run the tests. Then run the tests again.",
        json!({"phrases": [asked("run the tests")], "effects": []}),
    );
    assert_eq!(
        got.unwrap_err(),
        Refusal::Ambiguous {
            quote: "run the tests".into(),
            found: 2
        }
    );
}

#[test]
fn a_prefix_or_a_suffix_picks_one_of_two_places() {
    let source = "run the tests. Then run the tests again.";
    let by_prefix = json!({"quote": {"exact": "run the tests", "prefix": "Then "}});
    let by_suffix = json!({"quote": {"exact": "the tests", "suffix": "."}});
    let read = read(source, json!({"phrases": [by_prefix], "effects": []})).unwrap();
    assert_eq!(
        (read.phrases()[0].at.start(), read.phrases()[0].at.end()),
        (20, 33)
    );
    let read = super::tests::read(source, json!({"phrases": [by_suffix], "effects": []})).unwrap();
    assert_eq!(
        (read.phrases()[0].at.start(), read.phrases()[0].at.end()),
        (4, 13)
    );
}

#[test]
fn a_quote_not_in_the_source_is_refused_by_name() {
    let got = read(
        "Open a pull request.",
        json!({"phrases": [asked("merge it")], "effects": []}),
    );
    assert_eq!(
        got.unwrap_err(),
        Refusal::NotFound {
            quote: "merge it".into()
        }
    );
}

#[test]
fn a_quote_matches_after_white_space_normal_form_and_curly_quotes_are_folded() {
    let source =
        "Post one   comment\nthat says \u{201c}don\u{2019}t merge\u{201d} on the nai\u{308}ve.";
    let found = placed(
        source,
        &[
            asked("Post one comment that says \"don't merge\""),
            asked("na\u{ef}ve"),
        ],
    );
    assert_eq!(
        found,
        [
            "Post one   comment\nthat says \u{201c}don\u{2019}t merge\u{201d}",
            "nai\u{308}ve"
        ]
    );
}

#[test]
fn an_exact_match_wins_over_a_folded_one() {
    let source = "say \"hi\" or say \u{201c}hi\u{201d}";
    assert_eq!(placed(source, &[asked("say \"hi\"")]), ["say \"hi\""]);
}

#[test]
fn spans_in_non_ascii_text_fall_on_char_boundaries() {
    let source = "請先截圖 \u{1f4f8}，然後開一個拉取請求。";
    let read = read(
        source,
        json!({"phrases": [asked("截圖 \u{1f4f8}"), asked("開一個拉取請求")], "effects": []}),
    )
    .unwrap();
    for p in read.phrases() {
        assert!(source.is_char_boundary(p.at.start()) && source.is_char_boundary(p.at.end()));
    }
    let texts: Vec<_> = read
        .runs(source)
        .unwrap()
        .into_iter()
        .map(|(t, _)| t)
        .collect();
    assert_eq!(
        texts,
        ["請先", "截圖 \u{1f4f8}", "，然後", "開一個拉取請求", "。"]
    );
}

#[test]
fn a_folded_match_never_splits_a_char() {
    let source = "na\u{ef}ve tea";
    let got = read(
        source,
        json!({"phrases": [asked("\u{308}ve")], "effects": []}),
    );
    assert_eq!(
        got.unwrap_err(),
        Refusal::NotFound {
            quote: "\u{308}ve".into()
        }
    );
}

#[test]
fn an_empty_quote_is_refused() {
    let got = read("Open it.", json!({"phrases": [asked("  ")], "effects": []}));
    assert_eq!(got.unwrap_err(), Refusal::EmptyQuote { quote: "  ".into() });
}

#[test]
fn overlapping_phrases_are_refused_naming_both() {
    let got = read(
        "Take screenshots and post one comment.",
        json!({"phrases": [asked("screenshots and post"), asked("Take screenshots")], "effects": []}),
    );
    assert_eq!(
        got.unwrap_err(),
        Refusal::Overlap {
            first: "Take screenshots".into(),
            second: "screenshots and post".into()
        }
    );
}

#[test]
fn phrases_come_back_in_source_order_with_moor_ids() {
    let read = read(
        "First this. Then that.",
        json!({"phrases": [asked("Then that"), asked("First this")], "effects": []}),
    )
    .unwrap();
    let got: Vec<_> = read
        .phrases()
        .iter()
        .map(|p| (p.id.to_string(), p.quote.exact.clone()))
        .collect();
    assert_eq!(
        got,
        [
            ("p1".to_owned(), "First this".to_owned()),
            ("p2".to_owned(), "Then that".to_owned())
        ]
    );
}

fn shots_gate() -> serde_json::Value {
    json!({"kind": "gate", "label": "shots", "gate": {"name": "screenshots", "evidence": ["shot-*"]}})
}

fn comment_gate() -> serde_json::Value {
    json!({"kind": "gate", "label": "comment", "gate": {"name": "comment", "checks": ["test -f posted"]}})
}

pub(super) fn held(exact: &str, labels: &[&str]) -> serde_json::Value {
    json!({"quote": {"exact": exact}, "holds": labels})
}

const PR_SOURCE: &str = "Take screenshots of each page. Post one comment on the pull request.";

#[test]
fn an_effect_no_phrase_holds_is_refused_by_label() {
    let got = read(
        PR_SOURCE,
        json!({"phrases": [held("Take screenshots", &["shots"])], "effects": [shots_gate(), comment_gate()]}),
    );
    assert_eq!(
        got.unwrap_err(),
        Refusal::Unheld {
            label: "comment".into()
        }
    );
}

#[test]
fn a_phrase_naming_an_effect_not_handed_in_is_refused() {
    let got = read(
        PR_SOURCE,
        json!({"phrases": [held("Take screenshots", &["shots"])], "effects": []}),
    );
    assert_eq!(
        got.unwrap_err(),
        Refusal::UnknownEffect {
            quote: "Take screenshots".into(),
            label: "shots".into()
        }
    );
}

#[test]
fn two_effects_with_one_label_are_refused() {
    let mut again = comment_gate();
    again["label"] = json!("shots");
    let got = read(
        PR_SOURCE,
        json!({"phrases": [held("Take screenshots", &["shots"])], "effects": [shots_gate(), again]}),
    );
    assert_eq!(
        got.unwrap_err(),
        Refusal::DuplicateLabel {
            label: "shots".into()
        }
    );
}

#[test]
fn a_gate_that_does_not_parse_is_refused() {
    let bad = json!({"kind": "gate", "label": "shots", "gate": {"name": "Screenshots", "evidence": ["shot-*"]}});
    let got = read(
        PR_SOURCE,
        json!({"phrases": [held("Take screenshots", &["shots"])], "effects": [bad]}),
    );
    assert!(matches!(got.unwrap_err(), Refusal::Gates(_)));
}

#[test]
fn an_outcome_with_no_gate_is_refused() {
    let outcome = json!({"kind": "outcome", "label": "posted", "outcome": {"name": "posted"}});
    let got = read(
        PR_SOURCE,
        json!({"phrases": [held("Post one comment", &["posted"])], "effects": [outcome]}),
    );
    assert_eq!(
        got.unwrap_err(),
        Refusal::OutcomeWithoutGate {
            label: "posted".into()
        }
    );
}

#[test]
fn a_hand_in_to_a_reading_that_is_not_reading_is_refused() {
    let mut done = reading(PR_SOURCE);
    done.state = ReadingState::Failed {
        why: "quiet".into(),
        at: Ts::from_millis(2),
    };
    let got = hand_in(&done, &doc(json!({"phrases": [], "effects": []})));
    assert_eq!(got.unwrap_err(), Refusal::NotReading);
}

#[test]
fn the_hand_in_refuses_fields_it_does_not_know() {
    let got = serde_json::from_value::<HandIn>(json!({"phrases": [], "effects": [], "plan": {}}));
    assert!(got.is_err());
}

#[test]
fn the_hand_in_has_a_json_schema_for_its_tool() {
    let schema = serde_json::to_value(schemars::schema_for!(HandIn)).unwrap();
    let required = schema["required"].as_array().unwrap();
    assert!(required.contains(&json!("phrases")) && required.contains(&json!("effects")));
}
