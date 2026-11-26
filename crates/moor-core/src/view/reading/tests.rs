#![allow(clippy::indexing_slicing, reason = "test assertions index JSON values")]

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{Current, ReadingView};
use crate::gate::GatesDoc;
use crate::name::{BoxName, ForgeRepo};
use crate::reading::{
    Context, HandIn, ProxyAllow, ReaderRun, Reading, ReadingId, ReadingOf, ReadingState, RulesPin,
};
use crate::seam::clock::Ts;
use crate::user::UserId;
use crate::workflow::{PromptHash, WorkflowId};

const COMMENT: &str = "POST /repos/{repo}/issues/*/comments";
const PATCH: &str = "PATCH /repos/{repo}/issues/comments/*";
const STATUS: &str = "POST /repos/{repo}/statuses/*";

fn repos() -> Vec<ForgeRepo> {
    vec![ForgeRepo::new("acme", "web")]
}

fn pins() -> Vec<RulesPin> {
    vec![RulesPin {
        repo: Some(ForgeRepo::new("acme", "web")),
        revision: 3,
    }]
}

fn allow(entries: &[&str]) -> ProxyAllow {
    if entries.is_empty() {
        return BTreeMap::new();
    }
    BTreeMap::from([(
        "github-api".to_owned(),
        entries.iter().map(|e| (*e).to_owned()).collect(),
    )])
}

/// A reading of `source` that reader "claude" read, with moor's checks applied to `doc`.
fn read_of(source: &str, doc: &Value, allowed: &ProxyAllow) -> Reading {
    let reading = Reading {
        id: ReadingId::parse("01936a3e-7c2b-7d4e-8f00-0123456789ab").unwrap(),
        of: ReadingOf::Automation(WorkflowId::new(7)),
        parent: None,
        source: source.into(),
        context: Context {
            source_hash: PromptHash::of(source),
            repos: repos(),
            rules: pins(),
            instructions: 1,
        },
        made_by: UserId::new(1),
        state: ReadingState::Reading {
            by: ReaderRun {
                box_name: BoxName::parse("acme/reader-1").unwrap(),
                harness: "claude".into(),
                model: Some("opus".into()),
            },
            since: Ts::from_millis(1),
        },
    };
    let doc: HandIn = serde_json::from_value(doc.clone()).unwrap();
    let read = reading.hand_in(&doc, allowed).unwrap();
    Reading {
        state: ReadingState::Read(read),
        ..reading
    }
}

fn now<'a>(prompt: &'a str, allowed: &'a ProxyAllow) -> Current<'a> {
    Current {
        prompt,
        repos: &[],
        rules: &[],
        instructions: 1,
        allow: allowed,
    }
}

fn asked(exact: &str) -> Value {
    json!({"quote": {"exact": exact}, "mark": {"kind": "asked"}})
}

fn held(exact: &str, labels: &[&str]) -> Value {
    json!({"quote": {"exact": exact}, "mark": {"kind": "held", "effects": labels}})
}

fn comment(label: &str, entry: &str) -> Value {
    json!({"kind": "allow", "label": label, "rule": "github-api", "entry": entry})
}

const C1: &str = "Check out the pull request and run the repo’s check. Run the change and take screenshots of what it touches. Post one comment on the pull request and set the moor/evidence status; never push, approve or merge. Say plainly what you did not run.";

/// The C1 prompt of the portal's parity fixture, read the way the fixture draws it.
fn c1() -> (Reading, ProxyAllow) {
    let allowed = allow(&[COMMENT, PATCH, STATUS]);
    let doc = json!({
        "phrases": [
            asked("Check out the pull request and run the repo’s check."),
            asked("Run the change and take screenshots of what it touches."),
            held(
                "Post one comment on the pull request and set the moor/evidence status;",
                &["comment", "patch", "status"],
            ),
            asked("never push, approve or merge"),
            asked("Say plainly what you did not run."),
        ],
        "effects": [comment("comment", COMMENT), comment("patch", PATCH), comment("status", STATUS)],
    });
    (read_of(C1, &doc, &allowed), allowed)
}

/// What the portal's `ReadingViewSchema` parses for the C1 prompt in its parity fixture
/// (feat/reading-marks-portal, parity/fixtures/automation.cjs), with the differences the
/// controller's side has: the `key` is the effect's own key; `may` shows the allow lines the
/// effects hold; "never push, approve or merge" is asked, since slice 1 has no withheld effect;
/// and the spec's `id` and `reader` ride along, which the portal's schema ignores.
fn portal_c1() -> Value {
    json!({
        "id": "01936a3e-7c2b-7d4e-8f00-0123456789ab",
        "state": "read",
        "stale": [],
        "source": C1,
        "reader": {"harness": "claude", "model": "opus", "instructions": 1},
        "runs": [
            {"text": "Check out the pull request and run the repo’s check.", "phrase": "p1"},
            {"text": " "},
            {"text": "Run the change and take screenshots of what it touches.", "phrase": "p2"},
            {"text": " "},
            {"text": "Post one comment on the pull request and set the moor/evidence status;", "phrase": "p3"},
            {"text": " "},
            {"text": "never push, approve or merge", "phrase": "p4"},
            {"text": ". "},
            {"text": "Say plainly what you did not run.", "phrase": "p5"}
        ],
        "phrases": [
            {"id": "p1", "mark": "asked", "effects": []},
            {"id": "p2", "mark": "asked", "effects": []},
            {"id": "p3", "mark": "held", "effects": ["e1", "e2", "e3"]},
            {"id": "p4", "mark": "asked", "effects": []},
            {"id": "p5", "mark": "asked", "effects": []}
        ],
        "effects": [
            {"id": "e1", "kind": "allow", "line": COMMENT, "key": "allow:github-api:POST /repos/{repo}/issues/*/comments", "phrases": ["p3"]},
            {"id": "e2", "kind": "allow", "line": PATCH, "key": "allow:github-api:PATCH /repos/{repo}/issues/comments/*", "phrases": ["p3"]},
            {"id": "e3", "kind": "allow", "line": STATUS, "key": "allow:github-api:POST /repos/{repo}/statuses/*", "phrases": ["p3"]}
        ],
        "gates": null,
        "may": {"github-api": [COMMENT, PATCH, STATUS]}
    })
}

#[test]
fn a_reading_view_round_trips_the_fixture_json_the_portal_uses() {
    let (reading, allowed) = c1();
    let pinned = pins();
    let at = Current {
        repos: &repos(),
        rules: &pinned,
        ..now(C1, &allowed)
    };

    let view = ReadingView::of(&reading, &at);
    assert_eq!(serde_json::to_value(&view).unwrap(), portal_c1());

    let parsed: ReadingView = serde_json::from_value(portal_c1()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), portal_c1());
}

fn stored() -> Value {
    let doc: GatesDoc = serde_json::from_value(json!({
        "gates": [{"name": "proof", "evidence": ["pr-*"]}],
        "outcomes": [{"name": "Checked", "says": "the proof holds"}]
    }))
    .unwrap();
    serde_json::to_value(ReadingView::from_gates("show proof", &doc)).unwrap()
}

#[test]
fn stored_gates_with_no_reading_show_effects_and_no_phrases() {
    let v = stored();
    assert_eq!(v["phrases"], json!([]));
    assert_eq!(v["runs"], json!([{"text": "show proof"}]));
    assert_eq!(
        v["effects"][0],
        json!({"id": "e1", "kind": "gate", "line": "proof", "key": "gate:proof", "phrases": []})
    );
    assert_eq!(v["effects"][1]["key"], "outcome:checked");
    assert_eq!(v["effects"][1]["line"], "Checked");
    assert_eq!(v["gates"]["gates"][0]["name"], "proof");
    assert_eq!(v["may"], json!({}));
}

#[test]
fn a_view_of_stored_gates_has_no_reading_id_reader_or_stale_reason() {
    let v = stored();
    assert_eq!(v["id"], Value::Null);
    assert_eq!(v["state"], "read");
    assert_eq!(v["reader"], Value::Null);
    assert_eq!(v["stale"], json!([]));
}

#[test]
fn a_current_reading_is_not_stale() {
    let (reading, allowed) = c1();
    let pinned = pins();
    let at = Current {
        repos: &repos(),
        rules: &pinned,
        ..now(C1, &allowed)
    };
    assert!(ReadingView::of(&reading, &at).stale.is_empty());
}

#[test]
fn stale_lists_prompt_after_the_prompt_changes() {
    let (reading, allowed) = c1();
    let pinned = pins();
    let edited = format!("{C1} Then stop.");
    let at = Current {
        repos: &repos(),
        rules: &pinned,
        ..now(&edited, &allowed)
    };
    let v = serde_json::to_value(ReadingView::of(&reading, &at)).unwrap();
    assert_eq!(v["stale"], json!(["prompt"]));
}

#[test]
fn stale_lists_rules_after_a_rules_save() {
    let (reading, allowed) = c1();
    let saved = [RulesPin {
        revision: 4,
        ..pins().remove(0)
    }];
    let at = Current {
        repos: &repos(),
        rules: &saved,
        ..now(C1, &allowed)
    };
    let v = serde_json::to_value(ReadingView::of(&reading, &at)).unwrap();
    assert_eq!(v["stale"], json!(["rules"]));
}

#[test]
fn stale_lists_every_reason_that_applies_in_a_fixed_order() {
    let (reading, allowed) = c1();
    let at = Current {
        instructions: 2,
        repos: &[ForgeRepo::new("acme", "api")],
        ..now("something else", &allowed)
    };
    let v = serde_json::to_value(ReadingView::of(&reading, &at)).unwrap();
    assert_eq!(v["stale"], json!(["prompt", "repos", "rules", "reader"]));
}

#[test]
fn runs_concatenate_to_the_source_for_a_cjk_prompt() {
    let source = "运行检查。发布一条评论，然后停止。";
    let doc = json!({
        "phrases": [asked("运行检查。"), asked("发布一条评论")],
        "effects": []
    });
    let reading = read_of(source, &doc, &allow(&[]));

    let view = ReadingView::of(&reading, &now(source, &allow(&[])));

    let joined: String = view.runs.iter().map(|r| r.text.as_str()).collect();
    assert_eq!(joined, source);
    let phrased: Vec<_> = view
        .runs
        .iter()
        .filter_map(|r| Some((r.text.as_str(), r.phrase.as_deref()?)))
        .collect();
    assert_eq!(phrased, [("运行检查。", "p1"), ("发布一条评论", "p2")]);
}

#[test]
fn a_dangling_allow_effect_is_dropped_and_its_phrase_shows_asked() {
    let source = "Post one comment. Take screenshots, then set the status.";
    let doc = json!({
        "phrases": [
            held("Post one comment.", &["comment"]),
            held("Take screenshots, then set the status.", &["shots", "status"]),
        ],
        "effects": [
            comment("comment", COMMENT),
            {"kind": "gate", "label": "shots", "gate": {"name": "screenshots", "evidence": ["shot-*"]}},
            comment("status", STATUS),
        ],
    });
    let reading = read_of(source, &doc, &allow(&[COMMENT, STATUS]));

    // The config's proxy.allow lost the comment line and the status line.
    let view = ReadingView::of(&reading, &now(source, &allow(&[])));

    let v = serde_json::to_value(&view).unwrap();
    assert_eq!(
        v["phrases"][0],
        json!({"id": "p1", "mark": "asked", "effects": []})
    );
    assert_eq!(
        v["phrases"][1],
        json!({"id": "p2", "mark": "held", "effects": ["e2"]})
    );
    assert_eq!(v["effects"].as_array().map(Vec::len), Some(1));
    assert_eq!(v["effects"][0]["line"], "screenshots");
    assert_eq!(v["effects"][0]["phrases"], json!(["p2"]));
    assert_eq!(v["may"], json!({}));
}

#[test]
fn an_allow_entry_the_config_still_lists_stays_held() {
    let source = "Post one comment.";
    let doc = json!({
        "phrases": [held("Post one comment.", &["comment"])],
        "effects": [comment("comment", COMMENT)],
    });
    let reading = read_of(source, &doc, &allow(&[COMMENT]));

    let view = ReadingView::of(&reading, &now(source, &allow(&[COMMENT, STATUS])));

    let v = serde_json::to_value(&view).unwrap();
    assert_eq!(v["phrases"][0]["mark"], "held");
    assert_eq!(v["may"], json!({"github-api": [COMMENT]}));
}

#[test]
fn a_phrase_shows_the_reason_the_reader_gave() {
    let source = "Never push.";
    let doc = json!({"phrases": [{"quote": {"exact": "Never push."}, "mark": {"kind": "asked"},
                                  "why": "the rules let pushes through"}], "effects": []});
    let reading = read_of(source, &doc, &allow(&[]));
    let view = ReadingView::of(&reading, &now(source, &allow(&[])));
    assert_eq!(
        view.phrases[0].why.as_deref(),
        Some("the rules let pushes through")
    );
}

#[test]
fn phrases_that_do_not_fit_the_source_draw_no_marks() {
    let doc = json!({"phrases": [asked("run the check")], "effects": []});
    let reading = Reading {
        source: "run".into(),
        ..read_of("please run the check", &doc, &allow(&[]))
    };

    let view = ReadingView::of(&reading, &now("run", &allow(&[])));

    assert!(view.phrases.is_empty());
    let joined: String = view.runs.iter().map(|r| r.text.as_str()).collect();
    assert_eq!(joined, "run");
}

#[test]
fn a_failed_reading_shows_its_reason_and_no_marks() {
    let reading = Reading {
        state: ReadingState::Failed {
            why: "the quote \"Post\" was not found".into(),
            at: Ts::from_millis(2),
        },
        ..read_of(
            "Post one comment.",
            &json!({"phrases": [], "effects": []}),
            &allow(&[]),
        )
    };

    let v = serde_json::to_value(ReadingView::of(
        &reading,
        &now("Post one comment.", &allow(&[])),
    ))
    .unwrap();

    assert_eq!(v["state"], "failed");
    assert_eq!(v["failed"], "the quote \"Post\" was not found");
    assert_eq!(v["phrases"], json!([]));
    assert_eq!(v["runs"], json!([{"text": "Post one comment."}]));
}
