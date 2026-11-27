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

const FORBIDS: &str = "Never merge the pull request. Never push a branch.";

/// A reading of `FORBIDS` at `table`: never merge, which the rules refuse, and never push, which
/// they let through.
fn forbidding(table: u32) -> Reading {
    let mut reading = read(table);
    reading.source = FORBIDS.into();
    reading.context.source_hash = PromptHash::of(FORBIDS);
    reading.state = ReadingState::Reading {
        by: ReaderRun {
            box_name: BoxName::parse("acme/reader-1").unwrap(),
            harness: "claude".into(),
            model: None,
        },
        since: Ts::from_millis(1),
    };
    let doc: HandIn = serde_json::from_value(json!({
        "phrases": [
            {"quote": {"exact": "Never merge the pull request"}, "intents": [{"intent": "merge", "polarity": "never"}]},
            {"quote": {"exact": "Never push a branch"}, "intents": [{"intent": "push", "polarity": "never"}]}
        ],
        "effects": []
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

fn forbidding_view(reading: &Reading, table: Option<&IntentTable>) -> Value {
    let allowed = ProxyAllow::new();
    let at = Current {
        table,
        ..now(FORBIDS, &allowed)
    };
    serde_json::to_value(ReadingView::of(reading, &at)).unwrap()
}

#[test]
fn never_merge_shows_solid_with_its_two_withheld_requests() {
    let current = IntentTable::shipped().unwrap();
    let v = forbidding_view(&forbidding(current.version.0), Some(current));
    assert_eq!(v["phrases"][0]["mark"], "held");
    assert_eq!(v["phrases"][0]["effects"], json!(["e1", "e2"]));
    assert_eq!(v["phrases"][0]["intents"][0]["found"], "refused");
    assert_eq!(
        v["effects"],
        json!([
            {"id": "e1", "kind": "withheld", "line": "PUT /repos/{repo}/pulls/*/merge",
             "key": "withheld:api.github.com:PUT /repos/{repo}/pulls/*/merge", "phrases": ["p1"], "from": "table"},
            {"id": "e2", "kind": "withheld", "line": "POST /graphql mutation mergePullRequest",
             "key": "withheld:api.github.com:POST /graphql mutation mergePullRequest", "phrases": ["p1"], "from": "table"}
        ])
    );
    assert_eq!(v["may"], json!({}), "a withheld request is no may line");
}

#[test]
fn never_push_shows_dashed_naming_the_rule_that_lets_it_through() {
    let current = IntentTable::shipped().unwrap();
    let v = forbidding_view(&forbidding(current.version.0), Some(current));
    assert_eq!(v["phrases"][1]["mark"], "asked");
    assert_eq!(v["phrases"][1]["effects"], json!([]));
    assert_eq!(
        v["phrases"][1]["allowed"],
        json!({"rule": "github-git", "request": "POST /{repo}.git/git-receive-pack"})
    );
    assert_eq!(v["phrases"][1]["intents"][0]["found"], "let_through");
    assert!(v["phrases"][0].get("allowed").is_none());
}

#[test]
fn a_newer_table_that_forbids_other_requests_makes_a_forbidding_reading_stale() {
    let current = IntentTable::shipped().unwrap();
    let was = current.version.0;
    let same = IntentTable {
        version: TableVersion(was + 1),
        ..current.clone()
    };
    assert_eq!(
        forbidding_view(&forbidding(was), Some(&same))["stale"],
        json!([])
    );
    let mut fewer = same;
    for row in &mut fewer.intents {
        if row.name.as_str() == "merge" {
            row.never.truncate(1);
        }
    }
    assert_eq!(
        forbidding_view(&forbidding(was), Some(&fewer))["stale"],
        json!(["table"])
    );
}
