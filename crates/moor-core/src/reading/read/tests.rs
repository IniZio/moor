#![allow(clippy::indexing_slicing, reason = "test assertions")]

use serde_json::json;

use super::super::tests::{read, reader};
use super::super::{Change, EffectKey, Read};
use crate::gate::GatesDoc;

fn asked(exact: &str) -> serde_json::Value {
    json!({"quote": {"exact": exact}, "mark": {"kind": "asked"}})
}

fn held(exact: &str, labels: &[&str]) -> serde_json::Value {
    json!({"quote": {"exact": exact}, "mark": {"kind": "held", "effects": labels}})
}

fn gate(label: &str, name: &str, check: &str) -> serde_json::Value {
    json!({"kind": "gate", "label": label, "gate": {"name": name, "checks": [check]}})
}

#[test]
fn runs_reassemble_the_source_byte_for_byte() {
    let cases: [(&str, Vec<&str>); 5] = [
        ("Open a pull request.", vec![]),
        ("Open a pull request.", vec!["Open", "pull request."]),
        ("ab", vec!["a", "b"]),
        (
            "請先截圖 \u{1f4f8}，然後開一個拉取請求。",
            vec!["\u{1f4f8}", "開一個"],
        ),
        ("  tabs\tand\nlines  ", vec!["tabs\tand\nlines"]),
    ];
    for (source, quotes) in cases {
        let phrases: Vec<_> = quotes.iter().map(|q| asked(q)).collect();
        let read = read(source, json!({"phrases": phrases, "effects": []})).unwrap();
        let runs = read.runs(source).unwrap();
        assert_eq!(runs.iter().map(|(t, _)| *t).collect::<String>(), source);
        let marked: Vec<_> = runs
            .iter()
            .filter(|(_, p)| p.is_some())
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(marked, quotes, "{source}");
        assert!(runs.iter().all(|(t, _)| !t.is_empty()));
    }
}

#[test]
fn runs_refuse_a_source_the_phrases_were_not_placed_in() {
    let read = read(
        "Open a pull request.",
        json!({"phrases": [asked("pull request")], "effects": []}),
    )
    .unwrap();
    assert_eq!(read.runs("Open a PR."), None);
}

fn stored_gates() -> GatesDoc {
    serde_json::from_value(json!({
        "gates": [
            {"name": "screenshots", "evidence": ["shot-*"]},
            {"name": "review", "approval": "owner", "max_tries": 3},
        ],
        "outcomes": [{"name": "Comment posted", "checks": ["test -f done"]}],
    }))
    .unwrap()
}

#[test]
fn from_gates_round_trips_through_gates() {
    let doc = stored_gates();
    let read = Read::from_gates(&doc);
    assert!(read.phrases().is_empty());
    assert_eq!(read.reader(), None);
    assert_eq!(read.gates(), Some(doc));
}

#[test]
fn gates_keep_effect_order_and_are_none_without_a_gate() {
    let source = "Run lint, then the tests, and post one comment.";
    let both = read(
        source,
        json!({
            "phrases": [held("Run lint", &["lint"]), held("the tests", &["test"])],
            "effects": [gate("lint", "lint", "make lint"), gate("test", "test", "make test")],
        }),
    )
    .unwrap();
    let names: Vec<_> = both
        .gates()
        .unwrap()
        .gates
        .into_iter()
        .map(|g| g.name)
        .collect();
    assert_eq!(names, ["lint", "test"]);
    let allow_only = read(
        source,
        json!({
            "phrases": [held("post one comment", &["c"])],
            "effects": [{"kind": "allow", "label": "c", "rule": "github-app", "entry": "POST /repos/{repo}/issues/*/comments"}],
        }),
    )
    .unwrap();
    assert_eq!(allow_only.gates(), None);
}

#[test]
fn a_later_read_shows_each_effect_as_same_changed_or_new_and_lists_the_gone() {
    let source = "Run lint, then the tests, and say done.";
    let held_read = read(
        source,
        json!({
            "phrases": [held("Run lint", &["a", "b"]), held("say done", &["o"])],
            "effects": [
                gate("a", "lint", "make lint"),
                gate("b", "build", "make build"),
                {"kind": "outcome", "label": "o", "outcome": {"name": "Done"}},
            ],
        }),
    )
    .unwrap();
    let next = read(
        source,
        json!({
            "phrases": [held("Run lint", &["x"]), held("the tests", &["y"]), held("say done", &["z"])],
            "effects": [
                gate("x", "lint", "make lint"),
                gate("y", "test", "make test"),
                {"kind": "outcome", "label": "z", "outcome": {"name": "done", "says": "it is done"}},
            ],
        }),
    )
    .unwrap();
    let cmp = next.compare(&held_read);
    let each: Vec<_> = cmp
        .each
        .iter()
        .map(|(id, c)| (id.to_string(), *c))
        .collect();
    assert_eq!(
        each,
        [
            ("e1".to_owned(), Change::Same),
            ("e2".to_owned(), Change::New),
            ("e3".to_owned(), Change::Changed),
        ]
    );
    let gone: Vec<_> = cmp.gone.iter().map(super::Effect::key).collect();
    assert_eq!(gone, [EffectKey::Gate("build".into())]);
}

const SNAPSHOT: &str = r#"{
  "phrases": [
    {
      "id": "p1",
      "quote": {
        "exact": "Take screenshots"
      },
      "at": {
        "start": 0,
        "end": 16
      },
      "mark": {
        "kind": "held",
        "effects": [
          "e1"
        ]
      },
      "why": "a gate needs the files"
    },
    {
      "id": "p2",
      "quote": {
        "exact": "comment",
        "prefix": "one "
      },
      "at": {
        "start": 27,
        "end": 34
      },
      "mark": {
        "kind": "held",
        "effects": [
          "e2"
        ]
      }
    },
    {
      "id": "p3",
      "quote": {
        "exact": "soon"
      },
      "at": {
        "start": 35,
        "end": 39
      },
      "mark": {
        "kind": "asked"
      }
    }
  ],
  "effects": [
    {
      "kind": "gate",
      "id": "e1",
      "doc": {
        "name": "screenshots",
        "evidence": [
          "shot-*"
        ]
      }
    },
    {
      "kind": "allow",
      "id": "e2",
      "rule": "github-app",
      "entry": "POST /repos/{repo}/issues/*/comments"
    }
  ],
  "reader": {
    "box_name": "acme/reader-1",
    "harness": "claude",
    "model": "opus"
  }
}"#;

fn snapshot_read() -> Read {
    read(
        "Take screenshots. Post one comment soon.",
        json!({
            "phrases": [
                {"quote": {"exact": "Take screenshots"}, "mark": {"kind": "held", "effects": ["s"]}, "why": "a gate needs the files"},
                {"quote": {"exact": "comment", "prefix": "one "}, "mark": {"kind": "held", "effects": ["c"]}},
                asked("soon"),
            ],
            "effects": [
                {"kind": "gate", "label": "s", "gate": {"name": "screenshots", "evidence": ["shot-*"]}},
                {"kind": "allow", "label": "c", "rule": "github-app", "entry": "POST /repos/{repo}/issues/*/comments"},
            ],
        }),
    )
    .unwrap()
}

#[test]
fn a_read_is_stored_as_this_json_and_parses_back() {
    let read = snapshot_read();
    assert_eq!(serde_json::to_string_pretty(&read).unwrap(), SNAPSHOT);
    assert_eq!(serde_json::from_str::<Read>(SNAPSHOT).unwrap(), read);
    assert_eq!(read.reader(), Some(&reader()));
}

#[test]
fn stored_json_that_breaks_a_rule_does_not_parse() {
    let mut overlap: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    overlap["phrases"][1]["at"]["start"] = json!(10);
    let mut empty_held: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    empty_held["phrases"][0]["mark"]["effects"] = json!([]);
    let mut unheld: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    unheld["phrases"][1]["mark"] = json!({"kind": "asked"});
    let mut dangling: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    dangling["phrases"][1]["mark"]["effects"] = json!(["e9"]);
    for bad in [overlap, empty_held, unheld, dangling] {
        assert!(
            serde_json::from_value::<Read>(bad.clone()).is_err(),
            "{bad}"
        );
    }
}
