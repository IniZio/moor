#![allow(clippy::indexing_slicing, reason = "test assertions index JSON values")]

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::super::tests::now;
use super::super::{Current, ReadingView};
use crate::config::proxy::default_rules;
use crate::name::BoxName;
use crate::reading::{
    Context, HandIn, IntentTable, Pinned, ProxyAllow, ReaderRun, Reading, ReadingId, ReadingOf,
    ReadingState, TableVersion,
};
use crate::seam::clock::Ts;
use crate::user::UserId;
use crate::workflow::{PromptHash, WorkflowId};

const SOURCE: &str = "Take screenshots of each page. Post one comment on the pull request. Set the moor/evidence status.";
const POST: &str = "POST /repos/{repo}/issues/*/comments";
const PATCH: &str = "PATCH /repos/{repo}/issues/comments/*";
const GET: &str = "GET /repos/{repo}/issues/*/comments";

/// The intent reading of `SOURCE`: a gate, the comment row, and a status clause no row covers.
fn read(table: u32) -> Reading {
    let reading = Reading {
        id: ReadingId::parse("01936a3e-7c2b-7d4e-8f00-0123456789ab").unwrap(),
        of: ReadingOf::Automation(WorkflowId::new(7)),
        parent: None,
        source: SOURCE.into(),
        context: Context {
            source_hash: PromptHash::of(SOURCE),
            repos: Vec::new(),
            rules: Vec::new(),
            instructions: 1,
            table: Some(TableVersion(table)),
        },
        made_by: UserId::new(1),
        state: ReadingState::Reading {
            by: ReaderRun {
                box_name: BoxName::parse("acme/reader-1").unwrap(),
                harness: "claude".into(),
                model: None,
            },
            since: Ts::from_millis(1),
        },
    };
    let doc: HandIn = serde_json::from_value(json!({
        "phrases": [
            {"quote": {"exact": "Take screenshots of each page"}, "holds": ["shots"]},
            {"quote": {"exact": "Post one comment on the pull request"}, "intents": [{"intent": "comment", "polarity": "do"}]},
            {"quote": {"exact": "Set the moor/evidence status"}, "intents": [{"intent": "status", "polarity": "do"}]}
        ],
        "effects": [{"kind": "gate", "label": "shots", "gate": {"name": "screenshots", "evidence": ["shot-*"]}}]
    }))
    .unwrap();
    let rules = default_rules();
    let pinned = Pinned {
        rules: &rules,
        app: true,
    };
    let read = reading
        .hand_in(&doc, IntentTable::shipped().unwrap(), &pinned)
        .unwrap();
    Reading {
        state: ReadingState::Read(read),
        ..reading
    }
}

fn view(reading: &Reading, allowed: &ProxyAllow) -> Value {
    let at: Current<'_> = now(SOURCE, allowed);
    serde_json::to_value(ReadingView::of(reading, &at)).unwrap()
}

#[test]
fn a_phrase_shows_its_intent_with_the_lines_the_rules_already_allow() {
    let v = view(&read(1), &ProxyAllow::new());
    assert_eq!(v["table"], 1);
    assert_eq!(v["phrases"][1]["mark"], "held");
    assert_eq!(v["phrases"][1]["effects"], json!(["e2", "e3"]));
    assert_eq!(
        v["phrases"][1]["intents"],
        json!([{"intent": "comment", "polarity": "do", "repo": null, "as": "person",
                "found": "lines", "already": [GET], "row": true}])
    );
    assert_eq!(
        v["effects"][1],
        json!({"id": "e2", "kind": "allow", "line": POST,
        "key": format!("allow:github-api:{POST}"), "phrases": ["p2"], "from": "table"})
    );
    assert_eq!(v["effects"][2]["line"], PATCH);
    assert_eq!(v["may"], json!({"github-api": [POST, PATCH]}));
}

#[test]
fn a_clause_no_row_covers_shows_unmapped_and_holds_nothing() {
    let v = view(&read(1), &ProxyAllow::new());
    assert_eq!(v["phrases"][2]["mark"], "unmapped");
    assert_eq!(v["phrases"][2]["effects"], json!([]));
    assert_eq!(v["phrases"][2]["intents"][0]["found"], "no_row");
    assert_eq!(v["phrases"][2]["intents"][0]["row"], false);
}

#[test]
fn a_table_line_stays_whatever_the_config_lists() {
    let v = view(&read(1), &ProxyAllow::new());
    assert_eq!(v["effects"].as_array().map(Vec::len), Some(3));
}

#[test]
fn each_hand_written_line_is_tagged_against_the_reading() {
    let allowed: ProxyAllow = BTreeMap::from([
        (
            "github-api".to_owned(),
            vec![POST.to_owned(), "GET /repos/{repo}/pulls/*".to_owned()],
        ),
        (
            "github-app".to_owned(),
            vec![PATCH.to_owned(), "POST /repos/{repo}/statuses/*".to_owned()],
        ),
    ]);
    let v = view(&read(1), &allowed);
    assert_eq!(
        v["handwritten"],
        json!([
            {"rule": "github-api", "line": POST, "tag": "reproduced"},
            {"rule": "github-api", "line": "GET /repos/{repo}/pulls/*", "tag": "already_allowed"},
            {"rule": "github-app", "line": PATCH, "tag": "other_credential"},
            {"rule": "github-app", "line": "POST /repos/{repo}/statuses/*", "tag": "missed"}
        ])
    );
}

#[test]
fn a_newer_table_makes_a_reading_stale_only_when_it_changes_it() {
    let current = IntentTable::shipped().unwrap().version.0;
    assert_eq!(view(&read(current), &ProxyAllow::new())["stale"], json!([]));
    assert_eq!(
        view(&read(current - 1), &ProxyAllow::new())["stale"],
        json!(["table"]),
        "the status clause had no row"
    );
}
