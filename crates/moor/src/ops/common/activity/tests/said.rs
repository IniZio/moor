//! Tests for the `agent_said` event written just before a hook-reported `idled`.

use std::sync::Arc;

use moor_core::activity::{HookKind, HookRead, HookRecord};
use moor_core::BoxId;

use super::{cx, hook, rec, row, run_once, working_box, SESSION};
use crate::cx::Cx;
use crate::ops::common::activity::settle;
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
fn going_quiet_on_a_hook_records_the_agents_last_message_first() {
    let dir = tempfile::tempdir().unwrap();
    let mut probes = hook(HookKind::Idle);
    probes.hook = HookRead::Fresh(HookRecord {
        message: Some("Done.\nhttps://github.com/o/r/pull/7".to_owned()),
        ..rec(HookKind::Idle)
    });
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
    let mut probes = hook(HookKind::Idle);
    let long = format!("{}PR https://x/9", "a".repeat(20_000));
    probes.hook = HookRead::Fresh(HookRecord {
        message: Some(long),
        ..rec(HookKind::Idle)
    });
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
fn going_quiet_without_a_hook_message_says_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().probes(SESSION, hook(HookKind::Idle)));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    settle(&cx, cx.store().unwrap(), &row(&cx, id), &run_once(&cx, id)).unwrap();
    assert_eq!(
        said_then_idled(&cx, id),
        vec![("idled".to_owned(), serde_json::json!({}))]
    );
}
