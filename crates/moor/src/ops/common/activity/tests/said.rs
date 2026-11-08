//! Tests for the `agent_said` event written just before an `idled` the agent's own status gives.

use std::sync::Arc;

use moor_core::activity::Note;
use moor_core::BoxId;

use super::{cx, row, run_once, seen, working_box, SESSION};
use crate::cx::Cx;
use crate::ops::common::activity::settle;
use crate::testing::agent::{idle, said, with_note};
use crate::testing::engine::FakeEngine;

fn said_then_idled(cx: &Cx, id: BoxId) -> Vec<(String, serde_json::Value)> {
    let events = cx.store().unwrap().events(id, 100).unwrap();
    events
        .into_iter()
        .filter(|e| matches!(e.kind.as_str(), "agent_said" | "idled"))
        .map(|e| (e.kind, e.payload))
        .collect()
}

#[test]
fn going_quiet_on_the_status_records_the_agents_last_message_first() {
    let dir = tempfile::tempdir().unwrap();
    let probes = said(
        "Done.
https://github.com/o/r/pull/7",
        seen(),
    );
    let engine = Arc::new(FakeEngine::new().probes(SESSION, probes));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    let r = run_once(&cx, id);
    settle(&cx, cx.store().unwrap(), &row(&cx, id), &r).unwrap();
    settle(&cx, cx.store().unwrap(), &row(&cx, id), &run_once(&cx, id)).unwrap();
    let text = serde_json::json!({"text": "Done.\nhttps://github.com/o/r/pull/7"});
    assert_eq!(
        said_then_idled(&cx, id),
        vec![
            ("agent_said".to_owned(), text),
            ("idled".to_owned(), serde_json::json!({}))
        ]
    );
}

#[test]
fn a_long_last_message_keeps_its_end() {
    let dir = tempfile::tempdir().unwrap();
    let long = format!("{}PR https://x/9", "a".repeat(20_000));
    let probes = said(&long, seen());
    let engine = Arc::new(FakeEngine::new().probes(SESSION, probes));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    settle(&cx, cx.store().unwrap(), &row(&cx, id), &run_once(&cx, id)).unwrap();
    let said = said_then_idled(&cx, id);
    let text = said
        .first()
        .and_then(|(_, p)| p.get("text")?.as_str())
        .unwrap();
    assert!(text.ends_with("PR https://x/9"), "{text}");
    assert!(text.chars().count() <= 4000, "{}", text.len());
}

#[test]
fn going_quiet_without_a_said_note_says_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().probes(SESSION, idle(seen())));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    settle(&cx, cx.store().unwrap(), &row(&cx, id), &run_once(&cx, id)).unwrap();
    assert_eq!(
        said_then_idled(&cx, id),
        vec![("idled".to_owned(), serde_json::json!({}))]
    );
}

#[test]
fn a_waiting_agent_keeps_the_box_working_and_says_nothing_until_it_idles() {
    let dir = tempfile::tempdir().unwrap();
    let note = Note {
        said: Some("Watching.".to_owned()),
        background: vec!["gh run watch".to_owned()],
        ..Note::default()
    };
    let probes = with_note(idle(seen()), note);
    let engine = Arc::new(FakeEngine::new().probes(SESSION, probes));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    settle(&cx, cx.store().unwrap(), &row(&cx, id), &run_once(&cx, id)).unwrap();
    settle(&cx, cx.store().unwrap(), &row(&cx, id), &run_once(&cx, id)).unwrap();
    assert_eq!(said_then_idled(&cx, id), Vec::new());
    engine.set_probes(SESSION, said("Done.", seen()));
    settle(&cx, cx.store().unwrap(), &row(&cx, id), &run_once(&cx, id)).unwrap();
    settle(&cx, cx.store().unwrap(), &row(&cx, id), &run_once(&cx, id)).unwrap();
    assert_eq!(
        said_then_idled(&cx, id),
        vec![
            (
                "agent_said".to_owned(),
                serde_json::json!({"text": "Done."})
            ),
            ("idled".to_owned(), serde_json::json!({}))
        ]
    );
}

#[test]
fn a_long_last_message_keeps_its_start_too() {
    let dir = tempfile::tempdir().unwrap();
    let long = format!("Summary: disk full.\n{}PR https://x/9", "a".repeat(20_000));
    let probes = said(&long, seen());
    let engine = Arc::new(FakeEngine::new().probes(SESSION, probes));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    settle(&cx, cx.store().unwrap(), &row(&cx, id), &run_once(&cx, id)).unwrap();
    let said = said_then_idled(&cx, id);
    let text = said
        .first()
        .and_then(|(_, p)| p.get("text")?.as_str())
        .unwrap();
    assert!(text.starts_with("Summary: disk full.\n"), "{text}");
    assert!(text.ends_with("PR https://x/9"), "{text}");
}

#[test]
fn two_settles_from_one_read_record_the_message_once() {
    let dir = tempfile::tempdir().unwrap();
    let probes = said("Done.", seen());
    let engine = Arc::new(FakeEngine::new().probes(SESSION, probes));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    let stale = row(&cx, id);
    let r = run_once(&cx, id);
    settle(&cx, cx.store().unwrap(), &stale, &r).unwrap();
    settle(&cx, cx.store().unwrap(), &stale, &r).unwrap();
    let said: Vec<_> = said_then_idled(&cx, id)
        .into_iter()
        .filter(|(k, _)| k == "agent_said")
        .collect();
    assert_eq!(said.len(), 1, "{said:?}");
}
